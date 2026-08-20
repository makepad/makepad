//! Device-neutral op canary for the three graph shapes BS-RoFormer needs that
//! no other model in this workspace asks for.
//!
//! Each case builds a tiny graph in exactly the layout `graph.rs` produces,
//! runs it through whichever compiled-graph store `DeviceRuntime` binds, and
//! compares against a CPU reference computed here. Because the CPU reference
//! is the same code on both machines, running this on the Mac (Metal) and on a
//! CUDA box gives a real cross-store agreement check without moving a single
//! byte of fixture data between them.
//!
//! ```text
//! stems-ops-check              # bind the default device
//! MAKEPAD_AI_GRAPH_BACKEND=metal stems-ops-check
//! ```
//! Exit code is the number of failed cases (0 = all green).

use makepad_ai_common::backend::{BufferStorageMode, DeviceRuntime};
use makepad_ai_common::{
    BufferUsage, Context, Graph, InitParams, Op, Prec, TensorId, TensorType, GGML_ROPE_TYPE_NORMAL,
};
use makepad_ai_stems::config::{DIM_HEAD, HEADS, ROPE_THETA};

const ACT: BufferUsage = BufferUsage::Activations;

/// Deterministic, portable, and not a power of two anywhere near the tensor
/// shapes — so a stride bug cannot hide behind a repeating pattern.
struct Rng(u64);

impl Rng {
    fn next_f32(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let bits = (self.0 >> 33) as u32;
        (bits as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn vec(&mut self, n: usize) -> Vec<f32> {
        (0..n).map(|_| self.next_f32()).collect()
    }
}

fn as_bytes(values: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(values.as_ptr() as *const u8, values.len() * 4) }
}

fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

struct Report {
    failures: usize,
}

impl Report {
    /// Gated on SNR against the f64 reference, which is the crate's own
    /// contract language (the oracle-parity suite states ~60 dB on loud
    /// stems). A raw absolute tolerance would fail honest f32 accumulation
    /// order differences and pass a genuinely wrong kernel on a small-valued
    /// tensor; `max_abs` is reported alongside but does not decide.
    fn compare(&mut self, name: &str, got: &[f32], want: &[f32], min_snr_db: f32) {
        if got.len() != want.len() {
            println!("FAIL {name}: {} values, expected {}", got.len(), want.len());
            self.failures += 1;
            return;
        }
        let mut max_abs = 0.0f32;
        let mut at = 0usize;
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            if !g.is_finite() {
                println!("FAIL {name}: non-finite value {g} at {i}");
                self.failures += 1;
                return;
            }
            let diff = (g - w).abs();
            if diff > max_abs {
                max_abs = diff;
                at = i;
            }
        }
        let scale = want.iter().fold(0.0f32, |acc, v| acc.max(v.abs())).max(1e-6);
        let snr = signal_to_noise_db(got, want);
        if snr < min_snr_db {
            println!(
                "FAIL {name}: SNR {snr:.1} dB below {min_snr_db:.0} dB; worst |diff| {max_abs:.3e} \
                 at {at} (got {}, want {}), scale {scale:.3e}",
                got[at], want[at]
            );
            self.failures += 1;
        } else {
            println!(
                "ok   {name}: SNR {snr:.1} dB (>= {min_snr_db:.0}), max_abs {max_abs:.3e}, \
                 scale {scale:.3e}"
            );
        }
    }
}

fn signal_to_noise_db(got: &[f32], want: &[f32]) -> f32 {
    let mut signal = 0.0f64;
    let mut noise = 0.0f64;
    for (g, w) in got.iter().zip(want) {
        signal += (*w as f64) * (*w as f64);
        noise += ((*g - *w) as f64) * ((*g - *w) as f64);
    }
    if noise <= 0.0 {
        return f32::INFINITY;
    }
    (10.0 * (signal / noise).log10()) as f32
}

fn run_graph(
    runtime: &DeviceRuntime,
    ctx: &Context,
    outputs: &[TensorId],
    writes: &[(TensorId, &[u8])],
) -> Vec<Vec<f32>> {
    let mut graph = Graph::new();
    for &out in outputs {
        graph
            .build_forward_expand(ctx, out)
            .expect("graph build_forward_expand");
    }
    let session = runtime
        .compile_graph(
            ctx,
            &graph,
            outputs,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .expect("compile graph");
    let run = session.execute(ctx, writes, outputs).expect("execute graph");
    outputs
        .iter()
        .map(|id| from_bytes(run.outputs.get(id).expect("missing graph output")))
        .collect()
}

// ---------------------------------------------------------------------------
// Case 1: RoPE, GGML_ROPE_TYPE_NORMAL (interleaved adjacent pairs).
//
// Shape is the stems one: [dim_head, heads, seq, batch], rotated over ne[2].
// The NeoX/split-half kernel would produce a completely different tensor here
// while still looking plausible, which is exactly why this case exists.
// ---------------------------------------------------------------------------

fn case_rope_normal(runtime: &DeviceRuntime, report: &mut Report) {
    let (d, heads, seq, batch) = (DIM_HEAD, HEADS, 37usize, 5usize);
    let mut rng = Rng(0x5eed_1234);
    let src = rng.vec(d * heads * seq * batch);

    let mut ctx = Context::new(InitParams {
        mem_size: 64 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let x = ctx
        .new_named_tensor(
            "x".to_string(),
            TensorType::F32,
            4,
            &[d as i64, heads as i64, seq as i64, batch as i64],
            ACT,
        )
        .expect("x");
    ctx.write_tensor_data(x, as_bytes(&src)).expect("write x");
    let positions = ctx
        .new_named_tensor(
            "pos".to_string(),
            TensorType::I32,
            1,
            &[seq as i64],
            BufferUsage::Weights,
        )
        .expect("pos");
    let pos_values: Vec<i32> = (0..seq as i32).collect();
    let pos_bytes =
        unsafe { std::slice::from_raw_parts(pos_values.as_ptr() as *const u8, seq * 4) };
    ctx.write_tensor_data(positions, pos_bytes).expect("write pos");

    let out = ctx
        .rope(x, positions, d as i32, GGML_ROPE_TYPE_NORMAL, ACT)
        .expect("rope");
    ctx.set_no_alloc(true);

    let got = run_graph(runtime, &ctx, &[out], &[]).remove(0);

    // Reference: theta = pos * base^(-i0/n_dims), rotate (x[i0], x[i0+1]).
    let mut want = src.clone();
    for b in 0..batch {
        for s in 0..seq {
            let theta_base = s as f32;
            for h in 0..heads {
                let base = ((b * seq + s) * heads + h) * d;
                let mut i0 = 0usize;
                while i0 < d {
                    let theta = theta_base * ROPE_THETA.powf(-(i0 as f32) / d as f32);
                    let (sin_t, cos_t) = (theta.sin(), theta.cos());
                    let x0 = src[base + i0];
                    let x1 = src[base + i0 + 1];
                    want[base + i0] = x0 * cos_t - x1 * sin_t;
                    want[base + i0 + 1] = x0 * sin_t + x1 * cos_t;
                    i0 += 2;
                }
            }
        }
    }
    report.compare("rope_normal", &got, &want, 100.0);
}

// ---------------------------------------------------------------------------
// Case 2: maskless, non-causal, batched f32 attention.
//
// Built exactly as `graph.rs` builds it — a contiguous [d, heads, seq, batch]
// tensor, then `permute([0,2,1,3])` into flash_attn_ext — so the kernel sees
// the same strided (NOT contiguous) q/k/v views it sees in the real model, and
// writes the same transposed [d, heads, seq, batch] output.
// ---------------------------------------------------------------------------

fn case_attention(runtime: &DeviceRuntime, report: &mut Report) {
    // seq deliberately not a multiple of the kernel's 32-row tiling, and a
    // batch > 1 so a dropped ne[3] shows up as a gross mismatch.
    let (d, heads, seq, batch) = (DIM_HEAD, HEADS, 45usize, 3usize);
    let scale = 1.0 / (d as f32).sqrt();
    let mut rng = Rng(0xa11ce);
    let n = d * heads * seq * batch;
    let (qs, ks, vs) = (rng.vec(n), rng.vec(n), rng.vec(n));

    let mut ctx = Context::new(InitParams {
        mem_size: 128 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let dims = [d as i64, heads as i64, seq as i64, batch as i64];
    let mut make = |name: &str, values: &[f32]| -> TensorId {
        let id = ctx
            .new_named_tensor(name.to_string(), TensorType::F32, 4, &dims, ACT)
            .expect("operand");
        ctx.write_tensor_data(id, as_bytes(values)).expect("write");
        id
    };
    let q = make("q", &qs);
    let k = make("k", &ks);
    let v = make("v", &vs);

    let qa = ctx.permute(q, [0, 2, 1, 3]).expect("permute q");
    let ka = ctx.permute(k, [0, 2, 1, 3]).expect("permute k");
    let va = ctx.permute(v, [0, 2, 1, 3]).expect("permute v");
    let attn = ctx
        .flash_attn_ext(qa, ka, va, None, scale, 0.0, 0.0, ACT)
        .expect("flash_attn_ext");
    ctx.flash_attn_ext_set_prec(attn, Prec::F32).expect("prec");
    ctx.set_no_alloc(true);

    let got = run_graph(runtime, &ctx, &[attn], &[]).remove(0);

    // Reference: plain softmax attention per (batch, head), f64 accumulators.
    // Output index is ggml's: ((b * seq + s) * heads + h) * d + e.
    let mut want = vec![0.0f32; n];
    for b in 0..batch {
        for h in 0..heads {
            let at = |s: usize, e: usize| ((b * seq + s) * heads + h) * d + e;
            for s in 0..seq {
                let mut logits = vec![0.0f64; seq];
                for (t, logit) in logits.iter_mut().enumerate() {
                    let mut dot = 0.0f64;
                    for e in 0..d {
                        dot += qs[at(s, e)] as f64 * ks[at(t, e)] as f64;
                    }
                    *logit = dot * scale as f64;
                }
                let max = logits.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let mut sum = 0.0f64;
                for logit in logits.iter_mut() {
                    *logit = (*logit - max).exp();
                    sum += *logit;
                }
                for e in 0..d {
                    let mut acc = 0.0f64;
                    for (t, logit) in logits.iter().enumerate() {
                        acc += logit * vs[at(t, e)] as f64;
                    }
                    want[at(s, e)] = (acc / sum) as f32;
                }
            }
        }
    }
    report.compare("attention_batched_maskless", &got, &want, 90.0);
}

// ---------------------------------------------------------------------------
// Case 3: one 2D weight against a contiguous batched activation.
//
// Every matmul in the model has this shape. On CUDA it is routed to a single
// cuBLAS GEMM with the batch axes folded into the column count; the fold is
// only valid because both the activation and the destination are contiguous,
// so this case exists to catch a fold that silently transposed something.
// ---------------------------------------------------------------------------

fn case_batched_matmul(runtime: &DeviceRuntime, report: &mut Report) {
    let (k, n, m, batch) = (67usize, 41usize, 23usize, 7usize);
    let mut rng = Rng(0xbeef_00d);
    let weights = rng.vec(k * n);
    let acts = rng.vec(k * m * batch);

    let mut ctx = Context::new(InitParams {
        mem_size: 32 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let w = ctx
        .new_named_tensor(
            "w".to_string(),
            TensorType::F32,
            2,
            &[k as i64, n as i64],
            BufferUsage::Weights,
        )
        .expect("w");
    ctx.write_tensor_data(w, as_bytes(&weights)).expect("write w");
    let x = ctx
        .new_named_tensor(
            "x".to_string(),
            TensorType::F32,
            3,
            &[k as i64, m as i64, batch as i64],
            ACT,
        )
        .expect("x");
    ctx.write_tensor_data(x, as_bytes(&acts)).expect("write x");
    let out = ctx.mul_mat(w, x, ACT).expect("mul_mat");
    ctx.set_no_alloc(true);

    let got = run_graph(runtime, &ctx, &[out], &[]).remove(0);

    let mut want = vec![0.0f32; n * m * batch];
    for b in 0..batch {
        for col in 0..m {
            for row in 0..n {
                let mut acc = 0.0f64;
                for e in 0..k {
                    acc += weights[row * k + e] as f64 * acts[(b * m + col) * k + e] as f64;
                }
                want[(b * m + col) * n + row] = acc as f32;
            }
        }
    }
    report.compare("mul_mat_2d_weight_batched_acts", &got, &want, 80.0);
}

// ---------------------------------------------------------------------------
// Case 4: the elementwise broadcasts the trunk relies on.
//
// `rms_norm * gamma` broadcasts a [dim] vector over [dim, seq, batch], and the
// per-head attention gate broadcasts a [1, heads, seq, batch] sigmoid over a
// [dim_head, heads, seq, batch] tensor — a broadcast along dim 0, which is the
// one most kernels do not implement.
// ---------------------------------------------------------------------------

fn case_broadcasts(runtime: &DeviceRuntime, report: &mut Report) {
    let (d, heads, seq, batch) = (DIM_HEAD, HEADS, 11usize, 3usize);
    let mut rng = Rng(0xfeed_face);
    let n = d * heads * seq * batch;
    let x = rng.vec(n);
    let gates = rng.vec(heads * seq * batch);

    let mut ctx = Context::new(InitParams {
        mem_size: 32 << 20,
        mem_buffer: None,
        no_alloc: false,
    });
    let xt = ctx
        .new_named_tensor(
            "x".to_string(),
            TensorType::F32,
            4,
            &[d as i64, heads as i64, seq as i64, batch as i64],
            ACT,
        )
        .expect("x");
    ctx.write_tensor_data(xt, as_bytes(&x)).expect("write x");
    let gt = ctx
        .new_named_tensor(
            "g".to_string(),
            TensorType::F32,
            3,
            &[heads as i64, seq as i64, batch as i64],
            ACT,
        )
        .expect("g");
    ctx.write_tensor_data(gt, as_bytes(&gates)).expect("write g");
    let gr = ctx
        .reshape(gt, &[1, heads as i64, seq as i64, batch as i64])
        .expect("reshape gates");
    let out = ctx.binary_like_a(Op::Mul, xt, gr, ACT).expect("gate mul");
    ctx.set_no_alloc(true);

    let got = run_graph(runtime, &ctx, &[out], &[]).remove(0);

    let mut want = vec![0.0f32; n];
    for i in 0..n {
        want[i] = x[i] * gates[i / d];
    }
    report.compare("broadcast_ne0_gate", &got, &want, 120.0);
}

fn main() {
    let runtime = match DeviceRuntime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("stems-ops-check: no compiled-graph device: {err}");
            std::process::exit(3);
        }
    };
    println!(
        "device: {} ({})",
        runtime.description(),
        runtime.device().name()
    );

    let mut report = Report { failures: 0 };
    case_rope_normal(&runtime, &mut report);
    case_attention(&runtime, &mut report);
    case_batched_matmul(&runtime, &mut report);
    case_broadcasts(&runtime, &mut report);

    if report.failures == 0 {
        println!("stems-ops-check: all cases green");
    } else {
        println!("stems-ops-check: {} case(s) FAILED", report.failures);
    }
    std::process::exit(report.failures.min(2) as i32);
}
