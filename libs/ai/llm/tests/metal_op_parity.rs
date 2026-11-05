//! Metal-oracle parity for the ops whose CUDA validation is otherwise
//! self-referential (kernel checked against a reference transcribed from
//! the same kernel): run the exact seeded op cases through the REAL Metal
//! executor and compare against the shared CPU reference. A failure here
//! means the CPU reference (and therefore the CUDA implementation validated
//! against it) diverges from true ggml op semantics.
#![cfg(target_os = "macos")]

use makepad_ai_llm::metal_compiled::{
    execute_compiled_graph, prepare_graph, MetalGraphSession,
};
use makepad_ai_llm::{BufferUsage, Context, Graph, InitParams, Prec, TensorId, TensorType};
use makepad_ai_metal::{BufferStorageMode, MetalRuntime};

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 23) as f32) * 2.0 - 1.0
    }
}

fn f32s(rng: &mut Rng, n: usize) -> Vec<f32> {
    (0..n).map(|_| rng.f32()).collect()
}

fn as_bytes_f32(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn bytes_to_f32(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect()
}

struct MetalBench {
    ctx: Context,
}

impl MetalBench {
    fn new(bytes: usize) -> Self {
        Self {
            ctx: Context::new(InitParams {
                mem_size: bytes,
                mem_buffer: None,
                no_alloc: false,
            }),
        }
    }

    fn tensor(&mut self, name: &str, ty: TensorType, dims: &[i64], bytes: &[u8]) -> TensorId {
        let id = self
            .ctx
            .new_named_tensor(name.to_string(), ty, dims.len(), dims, BufferUsage::Weights)
            .expect("tensor alloc");
        let dst = self.ctx.tensor_data_mut(id).expect("tensor data");
        dst[..bytes.len()].copy_from_slice(bytes);
        id
    }

    fn run(&mut self, root: TensorId) -> Vec<f32> {
        // Parity must mean the ops actually ran: a silent empty-result skip
        // reported green while nothing executed. This suite is macOS-only,
        // where Metal init failing is a broken box, not a valid skip.
        let runtime = MetalRuntime::new().expect("Metal runtime for op parity");
        let mut graph = Graph::new();
        graph
            .build_forward_expand(&self.ctx, root)
            .expect("graph build");
        let prepared = prepare_graph(&self.ctx, &graph, runtime.features()).expect("prepare");
        let session = MetalGraphSession::from_runtime(
            runtime.clone(),
            &self.ctx,
            &prepared,
            BufferStorageMode::Private,
            BufferStorageMode::Private,
        )
        .expect("session");
        let execution =
            execute_compiled_graph(&runtime, &self.ctx, session.compiled(), &[], &[root])
                .expect("execute");
        bytes_to_f32(&execution.outputs[&root])
    }
}

fn assert_close(name: &str, got: &[f32], want: &[f32], tol_abs: f32, tol_rel: f32) {
    assert_eq!(got.len(), want.len(), "{name}: length mismatch");
    let mut worst = 0.0f32;
    let mut at = 0usize;
    for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
        let d = (g - w).abs() - (tol_abs + tol_rel * w.abs());
        // NaN compares false against everything, so an all-NaN output left
        // worst at 0.0 and the assert passed — the exact bug class this
        // suite exists to catch. Any non-finite deviation is a failure.
        assert!(
            d.is_finite(),
            "{name}: non-finite @ {i}: got {g}, want {w}"
        );
        if d > worst {
            worst = d;
            at = i;
        }
    }
    assert!(
        worst <= 0.0,
        "{name}: over tolerance @ {at}: got {}, want {}",
        got[at],
        want[at]
    );
}

/// Same case constants as llama-cuda-canary's gated_delta_net opcheck.
#[test]
fn metal_gated_delta_net_matches_shared_cpu_reference() {
    let (sv, h, hk, n_t) = (32usize, 6usize, 3usize, 3usize);
    let mut rng = Rng::new(4242);
    let q_data = f32s(&mut rng, sv * hk * n_t);
    let k_data = f32s(&mut rng, sv * hk * n_t);
    let v_data = f32s(&mut rng, sv * h * n_t);
    let g_data: Vec<f32> = f32s(&mut rng, h * n_t).iter().map(|x| -x.abs()).collect();
    let beta_data: Vec<f32> = f32s(&mut rng, h * n_t).iter().map(|x| 0.5 + 0.4 * x).collect();
    let state_data = f32s(&mut rng, sv * sv * h);

    let mut bench = MetalBench::new(8 << 20);
    let q = bench.tensor(
        "q",
        TensorType::F32,
        &[sv as i64, hk as i64, n_t as i64, 1],
        &as_bytes_f32(&q_data),
    );
    let k = bench.tensor(
        "k",
        TensorType::F32,
        &[sv as i64, hk as i64, n_t as i64, 1],
        &as_bytes_f32(&k_data),
    );
    let v = bench.tensor(
        "v",
        TensorType::F32,
        &[sv as i64, h as i64, n_t as i64, 1],
        &as_bytes_f32(&v_data),
    );
    let g = bench.tensor(
        "g",
        TensorType::F32,
        &[1, h as i64, n_t as i64, 1],
        &as_bytes_f32(&g_data),
    );
    let beta = bench.tensor(
        "beta",
        TensorType::F32,
        &[1, h as i64, n_t as i64, 1],
        &as_bytes_f32(&beta_data),
    );
    let state = bench.tensor(
        "state",
        TensorType::F32,
        &[(sv * sv) as i64, h as i64, 1, 1],
        &as_bytes_f32(&state_data),
    );
    let out = bench
        .ctx
        .gated_delta_net(q, k, v, g, beta, state, BufferUsage::Activations)
        .expect("gated_delta_net");
    let got = bench.run(out);

    // Shared CPU reference (same as the CUDA canary): sequential delta rule,
    // scalar gate (kda = 0), scale 1/sqrt(sv) on the attention output.
    let scale = 1.0 / (sv as f32).sqrt();
    let mut state_ref = state_data.clone();
    let mut attn_ref = vec![0.0f32; sv * h * n_t];
    for t in 0..n_t {
        for head in 0..h {
            let kv_head = head % hk;
            let qv = &q_data[(t * hk + kv_head) * sv..(t * hk + kv_head + 1) * sv];
            let kv = &k_data[(t * hk + kv_head) * sv..(t * hk + kv_head + 1) * sv];
            let vv = &v_data[(t * h + head) * sv..(t * h + head + 1) * sv];
            let g_scalar = g_data[t * h + head].exp();
            let beta_val = beta_data[t * h + head];
            for col in 0..sv {
                let scol = &mut state_ref[(head * sv + col) * sv..(head * sv + col + 1) * sv];
                let mut kv_dot = 0.0f32;
                for row in 0..sv {
                    kv_dot += scol[row] * kv[row];
                }
                let delta = (vv[col] - g_scalar * kv_dot) * beta_val;
                let mut attn = 0.0f32;
                for row in 0..sv {
                    scol[row] = g_scalar * scol[row] + kv[row] * delta;
                    attn += scol[row] * qv[row];
                }
                attn_ref[(t * h + head) * sv + col] = attn * scale;
            }
        }
    }
    let mut want = attn_ref;
    want.extend_from_slice(&state_ref);
    assert_close("metal_gdn", &got, &want, 5e-4, 1e-5);
}

/// Same case constants as llama-cuda-canary's ssm_conv opcheck.
#[test]
fn metal_ssm_conv_matches_shared_cpu_reference() {
    let (d_conv, d_inner, n_t) = (4usize, 16usize, 7usize);
    let span = n_t + d_conv - 1;
    let mut rng = Rng::new(777);
    let sx_data = f32s(&mut rng, span * d_inner);
    let c_data = f32s(&mut rng, d_conv * d_inner);
    let mut bench = MetalBench::new(4 << 20);
    let sx = bench.tensor(
        "sx",
        TensorType::F32,
        &[span as i64, d_inner as i64, 1],
        &as_bytes_f32(&sx_data),
    );
    let c = bench.tensor(
        "c",
        TensorType::F32,
        &[d_conv as i64, d_inner as i64],
        &as_bytes_f32(&c_data),
    );
    let out = bench
        .ctx
        .ssm_conv(sx, c, BufferUsage::Activations)
        .expect("ssm_conv");
    let got = bench.run(out);
    let mut want = vec![0.0f32; d_inner * n_t];
    for t in 0..n_t {
        for i in 0..d_inner {
            let mut acc = 0.0f32;
            for k in 0..d_conv {
                acc += sx_data[i * span + t + k] * c_data[i * d_conv + k];
            }
            want[t * d_inner + i] = acc;
        }
    }
    assert_close("metal_ssm_conv", &got, &want, 1e-5, 0.0);
}

/// A q8_0 KV cache is attended in place (the graph no longer casts it to
/// f16 first). The q8_0 flash-attention kernels must return exactly the bits
/// the f16 kernels return on the cache's f16 dequant, for the vec kernel
/// (decode, verify) and the simdgroup kernel (prefill chunks).
#[test]
fn metal_q8_0_kv_attention_matches_its_f16_dequant_bit_for_bit() {
    // (what, D, keys, queries, heads, KV heads); 24 over 4 at D=256 is the
    // production model.
    let cases = [
        ("decode step", 256i64, 1024i64, 1i64, 24i64, 4i64),
        ("decode step past 8k", 256, 8192, 1, 24, 4),
        ("verify width 4", 256, 1024, 4, 24, 4),
        ("prefill chunk", 256, 512, 32, 24, 4),
        ("D=128 decode step", 128, 512, 1, 16, 4),
        ("D=128 prefill chunk", 128, 512, 24, 16, 4),
    ];
    let runtime = MetalRuntime::new().expect("Metal runtime for op parity");
    let mut failures = Vec::new();
    for (index, &(what, d, kc, n_q, h, hkv)) in cases.iter().enumerate() {
        let mut rng = Rng::new(0x51ab + index as u64);
        let mut bench = MetalBench::new(160 << 20);
        let q_data = f32s(&mut rng, (d * n_q * h) as usize);
        let q = bench.tensor("q", TensorType::F32, &[d, n_q, h], &as_bytes_f32(&q_data));
        // Cache rows as the runtime keeps them, [D, Hkv, keys]: block scales
        // between 2^-8 and 2^-5 and random quants.
        let q8_cache = |rng: &mut Rng| -> Vec<u8> {
            let blocks = (d / 32 * hkv * kc) as usize;
            let mut bytes = Vec::with_capacity(blocks * 34);
            for _ in 0..blocks {
                let scale = 0x1C00 + (rng.next_u64() % 0x0C00) as u16;
                bytes.extend_from_slice(&scale.to_le_bytes());
                for _ in 0..32 {
                    bytes.push(((rng.next_u64() % 255) as i32 - 127) as i8 as u8);
                }
            }
            bytes
        };
        let k_bytes = q8_cache(&mut rng);
        let v_bytes = q8_cache(&mut rng);
        let k_cache = bench.tensor("k_cache", TensorType::Q8_0, &[d, hkv, kc], &k_bytes);
        let v_cache = bench.tensor("v_cache", TensorType::Q8_0, &[d, hkv, kc], &v_bytes);
        let mut mask_bytes = Vec::with_capacity((kc * n_q * 2) as usize);
        for iq in 0..n_q {
            for c in 0..kc {
                let hidden = c > kc - n_q + iq || rng.next_u64() % 9 == 0;
                mask_bytes.extend_from_slice(&if hidden { 0xFC00u16 } else { 0 }.to_le_bytes());
            }
        }
        let mask = bench.tensor("mask", TensorType::F16, &[kc, n_q], &mask_bytes);

        let ctx = &mut bench.ctx;
        let k = ctx.permute(k_cache, [0, 2, 1, 3]).unwrap();
        let v = ctx.permute(v_cache, [0, 2, 1, 3]).unwrap();
        let f16_copy = |ctx: &mut Context, src: TensorId| {
            let dst = ctx
                .new_tensor(TensorType::F16, 3, &[d, kc, hkv], BufferUsage::Activations)
                .unwrap();
            ctx.cpy(src, dst, BufferUsage::Activations).unwrap()
        };
        let k16 = f16_copy(ctx, k);
        let v16 = f16_copy(ctx, v);
        let scale = 1.0 / (d as f32).sqrt();
        let attend = |ctx: &mut Context, k: TensorId, v: TensorId| {
            let out = ctx
                .flash_attn_ext(q, k, v, Some(mask), scale, 0.0, 0.0, BufferUsage::Activations)
                .unwrap();
            ctx.flash_attn_ext_set_prec(out, Prec::F32).unwrap();
            out
        };
        let want = attend(ctx, k16, v16);
        let got = attend(ctx, k, v);

        let mut graph = Graph::new();
        graph.build_forward_expand(&bench.ctx, want).expect("graph build");
        graph.build_forward_expand(&bench.ctx, got).expect("graph build");
        let prepared = prepare_graph(&bench.ctx, &graph, runtime.features()).expect("prepare");
        let session = MetalGraphSession::from_runtime(
            runtime.clone(),
            &bench.ctx,
            &prepared,
            BufferStorageMode::Private,
            BufferStorageMode::Private,
        )
        .expect("session");
        let execution =
            execute_compiled_graph(&runtime, &bench.ctx, session.compiled(), &[], &[want, got])
                .expect("execute");
        let words = |b: &[u8]| -> Vec<u32> {
            b.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect()
        };
        let want = words(&execution.outputs[&want]);
        let got = words(&execution.outputs[&got]);
        assert!(
            want.iter().any(|&w| w != 0) && want.iter().all(|&w| f32::from_bits(w).is_finite()),
            "{what}: the f16 reference is degenerate"
        );
        let differ = want.iter().zip(&got).filter(|(a, b)| a != b).count();
        if differ != 0 || want.len() != got.len() {
            let first = want.iter().zip(&got).position(|(a, b)| a != b).unwrap_or(0);
            failures.push(format!(
                "{what}: {differ} of {} outputs differ from the f16 dequant's, first at {first}: {} vs {}",
                want.len(),
                f32::from_bits(got[first]),
                f32::from_bits(want[first])
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
