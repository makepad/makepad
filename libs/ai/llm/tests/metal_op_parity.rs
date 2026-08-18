//! Metal-oracle parity for the ops whose CUDA validation is otherwise
//! self-referential (kernel checked against a reference transcribed from
//! the same kernel): run the exact seeded op cases through the REAL Metal
//! executor and compare against the shared CPU reference. A failure here
//! means the CPU reference (and therefore the CUDA implementation validated
//! against it) diverges from true ggml op semantics.
#![cfg(target_os = "macos")]

use makepad_ggml::backend::metal::{
    execute_compiled_graph, prepare_graph, BufferStorageMode, MetalGraphSession, MetalRuntime,
};
use makepad_ggml::{BufferUsage, Context, Graph, InitParams, TensorId, TensorType};

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
