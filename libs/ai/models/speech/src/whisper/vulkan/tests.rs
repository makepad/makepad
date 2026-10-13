//! Real-device checks against the existing plain-Rust tensor/encoder/decoder.
//! Run with `cargo test --release -p makepad-ai-speech --lib vulkan_backend
//! -- --ignored --test-threads=1 --nocapture` for the opt-in comparisons.
//! Worker churn runs by default and skips only when no device is available.

use super::*;
use crate::whisper::{accel, decoder, encoder};
use crate::whisper::quant::{f32_to_f16, GGML_TYPE_Q8_0, GGML_TYPE_Q5_K, GGML_TYPE_Q4_K};
use makepad_ai_loader::{quant::{block_size, block_elements, TensorType}, quant_encode};
use crate::whisper::tensor::RawTensor;
use std::sync::{Mutex, MutexGuard};

static GPU: Mutex<()> = Mutex::new(());

struct Gpu(MutexGuard<'static, ()>);

impl Drop for Gpu {
    fn drop(&mut self) {
        RUNTIME.with(|s| *s.borrow_mut() = Runtime::New);
        TEST_VULKAN.with(|v| v.set(false));
        let _ = &self.0;
    }
}

fn gpu() -> Gpu {
    let guard = Gpu(GPU.lock().unwrap_or_else(|e| e.into_inner()));
    TEST_VULKAN.with(|v| v.set(true));
    assert!(is_available(), "a real Vulkan device and working kernels are required");
    guard
}

#[test]
fn runtime_survives_100_worker_threads() {
    if let Err(e) = makepad_ai_vulkan::Device::shared() {
        eprintln!("Whisper Vulkan worker churn skipped: no usable device ({e})");
        return;
    }
    for i in 0..100 {
        std::thread::spawn(move || {
            TEST_VULKAN.with(|v| v.set(true));
            assert!(is_available(), "Whisper Vulkan runtime {i} failed to initialize");
            // Leave RUNTIME populated: thread exit must drop its streams,
            // buffers and pipelines without destroying the shared device.
        }).join().unwrap_or_else(|_| panic!("Whisper Vulkan worker {i} panicked"));
    }
}

#[test]
#[ignore = "requires MAKEPAD_WHISPER_TEST_MODEL and a real Vulkan device"]
fn model_load_survives_100_worker_threads() {
    if let Err(e) = makepad_ai_vulkan::Device::shared() {
        eprintln!("Whisper Vulkan model churn skipped: no usable device ({e})");
        return;
    }
    let path = std::env::var("MAKEPAD_WHISPER_TEST_MODEL").expect("set MAKEPAD_WHISPER_TEST_MODEL to a Whisper model file");
    for i in 0..100 {
        let path = path.clone();
        std::thread::spawn(move || {
            TEST_VULKAN.with(|v| v.set(true));
            let model = WhisperModel::load_file(&path).expect("load Whisper model");
            run("model churn", |ctx| ctx.model(&model).map(|_| ()))
                .unwrap_or_else(|| panic!("Whisper Vulkan model load {i} failed"));
        }).join().unwrap_or_else(|_| panic!("Whisper Vulkan model worker {i} panicked"));
    }
}

fn cpu<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) { accel::TEST_CPU.with(|v| v.set(self.0)); }
    }
    let _reset = Reset(accel::TEST_CPU.with(|v| v.replace(true)));
    f()
}

fn values(n: usize, seed: u32) -> Vec<f32> {
    let mut rng = seed;
    (0..n).map(|_| {
        rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
        ((rng >> 8) as f32 / 16777216.0 - 0.5) * 2.0
    }).collect()
}

fn tensor(shape: &[usize], seed: u32) -> Tensor {
    Tensor { data: values(shape.iter().product(), seed), shape: shape.to_vec() }
}

fn raw(data: &[f32], shape: &[usize], half: bool) -> RawTensor {
    RawTensor {
        data: if half { data.iter().flat_map(|&v| f32_to_f16(v).to_le_bytes()).collect::<Vec<u8>>().into() }
            else { bytes(data).to_vec().into() },
        shape: shape.to_vec(),
        ggml_type: if half { GGML_TYPE_F16 } else { GGML_TYPE_F32 },
    }
}

fn quantized(data: &[f32], shape: &[usize], ty: u32) -> RawTensor {
    let cols = shape[..shape.len() - 1].iter().product::<usize>();
    let ty = if cols % block_elements(ty) != 0 { GGML_TYPE_Q8_0 } else { ty };
    let kind = match ty { GGML_TYPE_Q8_0 => TensorType::Q8_0, GGML_TYPE_Q5_K => TensorType::Q5K,
        GGML_TYPE_Q4_K => TensorType::Q4K, _ => unreachable!() };
    let mut bytes = vec![0; data.len() / block_elements(ty) * block_size(ty)];
    quant_encode::encode(kind, data, &mut bytes).unwrap();
    RawTensor { data: bytes.into(), shape: shape.to_vec(), ggml_type: ty }
}

// The integer kernels consume per-block rounded activations, with f32 scales.
fn rounded_activations(x: &Tensor) -> Tensor {
    let mut out = x.clone();
    let cols = *x.shape.last().unwrap();
    for row in out.data.chunks_mut(cols) {
        for block in row.chunks_mut(32) {
            let d = block.iter().map(|v| v.abs()).fold(0.0f32, f32::max) / 127.0;
            let inv = if d == 0.0 { 0.0 } else { 1.0 / d };
            for v in block { *v = (*v * inv).round() * d; }
        }
    }
    out
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn resident_quantized_matrices_match_cpu() {
    let _gpu = gpu();
    for ty in [GGML_TYPE_Q8_0, GGML_TYPE_Q5_K, GGML_TYPE_Q4_K] {
        for (m, k, n) in [(1usize, 32usize, 7usize), (4, 96, 5), (5, 32, 3), (31, 256, 7), (32, 256, 9), (67, 96, 65), (5, 1280, 9)] {
            let k = if ty == GGML_TYPE_Q8_0 { k } else { k.div_ceil(256) * 256 };
            let x = tensor(&[m, k], 73);
            let w = quantized(&values(n * k, 91), &[k, n], ty);
            let bias = tensor(&[n], 43);
            let wf = w.to_f32();
            let want = cpu(|| Tensor::matmul_t(&rounded_activations(&x), &wf));
            let got = try_matmul_nt_ggml_bytes(&x.data, &w.data, ty, m, k, n).unwrap();
            close("resident quantised matrix", &got, &want.data, 4e-5);
            let mut want = want.data;
            for (i, v) in want.iter_mut().enumerate() { *v += bias.data[i % n]; }
            let got = try_matmul_nt_ggml_bytes_add_bias(&x.data, &w.data, ty, m, k, n, &bias.data).unwrap();
            close("resident quantised matrix with bias", &got, &want, 4e-5);
            run("quantised residency", |ctx| {
                let l = Linear::encoder(&ctx.ops.device, &w, None, true)?;
                assert!(l.w.quant.is_some());
                assert!(l.w.int8.is_none(), "no second weight representation");
                assert_eq!(l.w.buffer.size, l.w.quant.unwrap().row_bytes(k as u32) * n as u64);
                Ok(())
            }).unwrap();
        }
    }
}

fn close(name: &str, actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len(), "{name}: shape");
    let mut max_error = 0.0f32;
    for (i, (&a, &b)) in actual.iter().zip(expected).enumerate() {
        let error = (a - b).abs();
        assert!(a.is_finite() && b.is_finite() && error <= tolerance * (1.0 + b.abs()),
            "{name}[{i}]: {a} vs {b}, error {error}, tolerance {tolerance}");
        max_error = max_error.max(error);
    }
    eprintln!("{name}: {} elements, max absolute error {max_error:.7}", actual.len());
}

// A whole quantised stack rounds different intermediate activations than
// the float CPU stack. Kernel/projection checks above compare identical
// rounded inputs tightly; here measure accumulated error across the stack.
fn close_stack(name: &str, actual: &[f32], expected: &[f32], quantised: bool) {
    if !quantised { return close(name, actual, expected, 2e-4); }
    assert_eq!(actual.len(), expected.len(), "{name}: shape");
    let error: f64 = actual.iter().zip(expected).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum();
    let norm: f64 = expected.iter().map(|&b| (b as f64).powi(2)).sum();
    let relative = (error / norm.max(f64::MIN_POSITIVE)).sqrt();
    assert!(relative <= 0.01, "{name}: accumulated relative L2 error {relative}");
    eprintln!("{name}: relative L2 error {relative:.7}");
    close(name, actual, expected, 0.02);
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn matrices_match_cpu() {
    let _gpu = gpu();
    // Include both dispatch families, unaligned rows, reduction tails, and
    // more than one workgroup in each output dimension.
    for (m, k, n) in [(1, 65, 7), (4, 1280, 9), (5, 37, 35), (33, 64, 37), (35, 35, 33), (71, 67, 69)] {
        let x = tensor(&[m, k], 17);
        let weights = values(n * k, 29);
        let bias = tensor(&[n], 43);
        for half in [false, true] {
            let w = raw(&weights, &[k, n], half);
            let want = cpu(|| Tensor::linear_raw(&x, &w, &bias));
            let got = try_matmul_nt_ggml_bytes_add_bias(&x.data, &w.data, w.ggml_type, m, k, n, &bias.data).unwrap();
            close(&format!("matrix {m}x{k}x{n} half={half}"), &got, &want.data, 3e-5);
            let want = cpu(|| Tensor::matmul_raw(&x, &w));
            let got = try_matmul_nt_ggml_bytes(&x.data, &w.data, w.ggml_type, m, k, n).unwrap();
            close("matrix without bias", &got, &want.data, 3e-5);
        }
    }
    let x = tensor(&[7, 64], 73);
    let w = raw(&values(64 * 37, 91), &[64, 37], false).to_q8_0();
    // Compare the same weight values and per-block rounded activations.
    let wf = w.to_f32();
    let want = cpu(|| Tensor::matmul_t(&rounded_activations(&x), &wf));
    let got = try_matmul_nt_ggml_bytes(&x.data, &w.data, w.ggml_type, 7, 64, 37).unwrap();
    close("quantized matrix", &got, &want.data, 3e-5);
    let b = tensor(&[64, 9], 12);
    close("NN matrix", &try_matmul_nn_f32(&x.data, &b.data, 7, 64, 9).unwrap(),
        &cpu(|| Tensor::matmul(&x, &b)).data, 3e-5);
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn elementwise_and_norm_match_cpu() {
    let _gpu = gpu();
    for cols in [1, 37, 129, 1280] {
        let x = tensor(&[3, cols], 24);
        let mul = tensor(&[cols], 37);
        let add = tensor(&[cols], 42);
        for b in [tensor(&[1], 6), tensor(&[cols], 12), tensor(&[3, cols], 18)] {
            close("add", &try_add_f32(&x.data, &x.shape, &b.data, &b.shape).unwrap(),
                &cpu(|| Tensor::add(&x, &b)).data, 1e-6);
            close("multiply", &try_mul_f32(&x.data, &x.shape, &b.data, &b.shape).unwrap(),
                &cpu(|| Tensor::mul(&x, &b)).data, 1e-6);
        }
        close("GELU", &try_gelu_f32(&x.data, &x.shape).unwrap(),
            &cpu(|| Tensor::gelu(&x)).data, 2e-6);
        close("norm", &try_layer_norm_f32(&x.data, &x.shape, 1e-5).unwrap(),
            &cpu(|| Tensor::layer_norm(&x, 1e-5)).data, 3e-6);
        close("affine norm", &try_layer_norm_mul_add_f32(&x.data, &x.shape,
            &mul.data, &mul.shape, &add.data, &add.shape, 1e-5).unwrap(),
            &cpu(|| Tensor::layer_norm_mul_add(&x, &mul, &add, 1e-5)).data, 3e-6);
    }
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn convolutions_match_cpu() {
    let _gpu = gpu();
    for (ic, iw, oc, stride, ty) in [(3, 67, 9, 1, None), (8, 66, 35, 2, None),
        (32, 67, 9, 1, Some(GGML_TYPE_Q8_0)), (256, 66, 35, 2, Some(GGML_TYPE_Q5_K)),
        (256, 67, 9, 2, Some(GGML_TYPE_Q4_K))] {
        let x = tensor(&[ic, iw], 82);
        let data = values(3 * ic * oc, 97);
        let weights = if let Some(ty) = ty { quantized(&data, &[3, ic, oc], ty) }
            else { raw(&data, &[3, ic, oc], true) };
        let bias = tensor(&[oc], 43);
        let want = cpu(|| Tensor::transpose_2d(&Tensor::conv1d_raw(&x, &weights, &bias, stride)));
        let ow = want.shape[0];
        let want = if ty.is_some() {
            let mut cols = Tensor { data: vec![0.0; ow * ic * 3], shape: vec![ow, ic * 3] };
            for t in 0..ow {
                for c in 0..ic {
                    for d in 0..3 {
                        let pos = (t * stride + d) as isize - 1;
                        if pos >= 0 && pos < iw as isize {
                            cols.data[t * ic * 3 + c * 3 + d] = x.data[c * iw + pos as usize];
                        }
                    }
                }
            }
            let w = RawTensor { data: weights.data.to_vec().into(), shape: vec![ic * 3, oc], ggml_type: weights.ggml_type };
            cpu(|| Tensor::linear_raw(&rounded_activations(&cols), &raw(&w.to_f32().data, &[ic * 3, oc], false), &bias))
        } else { want };
        for time_major in [false, true] {
            let input = if time_major { Tensor::transpose_2d(&x) } else { x.clone() };
            let got = run("convolution test", |ctx| {
                let op = &mut ctx.ops;
                let w = Linear::raw(&op.device, &weights.data, weights.ggml_type, oc, ic * 3, Some(&bias.data))?;
                let mut plan = Layout::default();
                let src = plan.take(input.data.len())?;
                let col = plan.take(ow * ic * 3)?;
                let dst = plan.take(ow * oc)?;
                op.reserve(&plan)?;
                op.write(src, &input.data)?;
                op.begin()?;
                op.im2col(src, col, ic, iw, 3, stride, 1, time_major);
                op.linear(col, dst, ow, &w)?;
                op.finish()?;
                op.read(dst)
            }).unwrap();
            close("conv1d", &got, &want.data, 3e-5);
        }
    }
}

fn attention_cpu(q: &[f32], k: &[f32], v: &[f32], nq: usize, nk: usize, heads: usize, past: Option<usize>) -> Vec<f32> {
    cpu(|| {
        let width = heads * 64;
        let mut out = vec![0.0; nq * width];
        for h in 0..heads {
            let head = |data: &[f32], rows: usize| Tensor {
                data: (0..rows).flat_map(|r| data[r * width + h * 64..r * width + (h + 1) * 64].iter().copied()).collect(),
                shape: vec![rows, 64],
            };
            let mut scores = Tensor::matmul_t(&head(q, nq), &head(k, nk));
            if let Some(past) = past {
                for row in 0..nq {
                    for col in past + row + 1..nk { scores.data[row * nk + col] = f32::NEG_INFINITY; }
                }
            }
            let probs = Tensor::softmax(&scores, None, 0.125);
            let result = Tensor::matmul(&probs, &head(v, nk));
            for r in 0..nq {
                out[r * width + h * 64..r * width + (h + 1) * 64].copy_from_slice(&result.data[r * 64..(r + 1) * 64]);
            }
        }
        out
    })
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn attention_matches_cpu() {
    let _gpu = gpu();
    for (nq, nk, heads, past) in [(1, 1, 2, None), (9, 37, 2, None), (35, 65, 3, Some(30)),
        (9, 9, 2, Some(0)), (1, 1500, 2, None), (1500, 1500, 1, None)] {
        let q = values(nq * heads * 64, 15);
        let k = values(nk * heads * 64, 64);
        let v = values(nk * heads * 64, 92);
        let want = attention_cpu(&q, &k, &v, nq, nk, heads, past);
        let got = run("attention test", |ctx| {
            let op = &mut ctx.ops;
            let mut plan = Layout::default();
            let query = plan.take(q.len())?;
            let key = plan.take(k.len())?;
            let val = plan.take(v.len())?;
            let dst = plan.take(q.len())?;
            op.reserve(&plan)?;
            op.write(query, &q)?; op.write(key, &k)?; op.write(val, &v)?;
            op.begin()?;
            op.attention(op.addr(query), op.addr(key), op.addr(val), op.addr(dst), nq, nk, heads, past, 0.125);
            op.finish()?;
            op.read(dst)
        }).unwrap();
        close(&format!("attention {nq}x{nk} heads={heads} past={past:?}"), &got, &want, 5e-6);
        if past.is_none() {
            close("packed attention", &try_flash_attn_f32_packed(&q, &k, &v, nq, nk, heads, 64, 0.125).unwrap(), &want, 5e-6);
            close("cross padding ignored", &try_flash_attn_f32_cross_kv_cache(0, &q, &k, &v, nq, nk.div_ceil(256) * 256, heads, 64, 0.125).unwrap(), &want, 5e-6);
        }
    }
}

// A small deterministic model in the existing file format exercises the real
// loader and complete CPU paths without model files or downloads. It has the
// production head width, two layers, F16 matrices, and partial attention tiles.
fn model(seed: u32) -> WhisperModel {
    model_with_quant(seed, None)
}

fn model_with_quant(seed: u32, ty: Option<u32>) -> WhisperModel {
    let mut file = Vec::new();
    for v in [0x67676d6c_i32, 97, 37, 128, 2, 2, 64, 128, 2, 2, 8, 1, 0, 0, 0] {
        file.extend_from_slice(&v.to_le_bytes());
    }
    let mut serial = seed;
    let mut weight = |name: &str, shape: &[usize], half: bool, norm: bool| {
        serial = serial.wrapping_add(13);
        let mut data = values(shape.iter().product(), serial);
        let scale = if shape.len() > 1 { 0.4 / (shape[0] as f32).sqrt() } else { 0.03 };
        for v in &mut data { *v = *v * scale + if norm { 1.0 } else { 0.0 }; }
        let w = if let Some(ty) = ty.filter(|_| shape.len() == 2 && name.ends_with(".weight") && !name.contains("embedding")) {
            quantized(&data, shape, ty)
        } else { raw(&data, shape, half) };
        for v in [shape.len() as u32, name.len() as u32, w.ggml_type] { file.extend_from_slice(&v.to_le_bytes()); }
        for &v in shape { file.extend_from_slice(&(v as u32).to_le_bytes()); }
        file.extend_from_slice(name.as_bytes());
        file.extend_from_slice(&w.data);
    };
    weight("encoder.positional_embedding", &[128, 37], false, false);
    weight("encoder.conv1.weight", &[3, 8, 128], true, false);
    weight("encoder.conv1.bias", &[128], false, false);
    weight("encoder.conv2.weight", &[3, 128, 128], true, false);
    weight("encoder.conv2.bias", &[128], false, false);
    weight("encoder.ln_post.weight", &[128], false, true);
    weight("encoder.ln_post.bias", &[128], false, false);
    weight("decoder.positional_embedding", &[128, 64], false, false);
    weight("decoder.token_embedding.weight", &[128, 97], true, false);
    weight("decoder.ln.weight", &[128], false, true);
    weight("decoder.ln.bias", &[128], false, false);
    for side in ["encoder", "decoder"] {
        for i in 0..2 {
            let base = format!("{side}.blocks.{i}");
            for attn in if side == "decoder" { & ["attn", "cross_attn"][..] } else { &["attn"][..] } {
                weight(&format!("{base}.{attn}_ln.weight"), &[128], false, true);
                weight(&format!("{base}.{attn}_ln.bias"), &[128], false, false);
                for proj in ["query", "key", "value", "out"] {
                    weight(&format!("{base}.{attn}.{proj}.weight"), &[128, 128], true, false);
                    if proj != "key" { weight(&format!("{base}.{attn}.{proj}.bias"), &[128], false, false); }
                }
            }
            weight(&format!("{base}.mlp_ln.weight"), &[128], false, true);
            weight(&format!("{base}.mlp_ln.bias"), &[128], false, false);
            weight(&format!("{base}.mlp.0.weight"), &[128, 512], true, false);
            weight(&format!("{base}.mlp.0.bias"), &[512], false, false);
            weight(&format!("{base}.mlp.2.weight"), &[512, 128], true, false);
            weight(&format!("{base}.mlp.2.bias"), &[128], false, false);
        }
    }
    WhisperModel::load(&mut std::io::Cursor::new(file)).unwrap()
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn fused_encoder_decoder_and_cache_match_cpu() {
    let _gpu = gpu();
    for (seed, ty) in [(13, None), (37, None), (53, Some(GGML_TYPE_Q8_0)),
        (61, Some(GGML_TYPE_Q5_K)), (71, Some(GGML_TYPE_Q4_K))] {
        let model = model_with_quant(seed, ty);
        let mel = values(8 * 74, seed);
        let want = cpu(|| encoder::encode(&model, &mel, 37));
        let encoded = try_encode(&model, &mel, 37).unwrap();
        close_stack("complete encoder", &encoded, &want.data, ty.is_some());
        let input = Tensor { data: encoded, shape: vec![37, 128] };
        let cross_input = if ty.is_some() { rounded_activations(&input) } else { input.clone() };
        let want_cross = cpu(|| encoder::compute_cross_kv(&model, &cross_input));
        let cross = try_cross_kv(&model, &input).unwrap();
        for ((k, v), (wk, wv)) in cross.iter().zip(&want_cross) {
            close("cross K", &k.data, &wk.data, 2e-5);
            close("cross V", &v.data, &wv.data, 2e-5);
        }
        let mut reference = KvCache::new(2);
        let mut cache = KvCache::new(2);
        for (tokens, cpu_step) in [(&[3, 7, 5, 1, 8, 9, 2][..], false), (&[9][..], false), (&[4, 8][..], true), (&[6][..], false)] {
            let positions: Vec<_> = (cache.n_past..cache.n_past + tokens.len()).map(|p| p as i32).collect();
            let want = cpu(|| decoder::decode(&model, tokens, &positions, &mut reference, &cross, None));
            let got = if cpu_step {
                cpu(|| decoder::decode(&model, tokens, &positions, &mut cache, &cross, None)).data
            } else { try_decode(&model, tokens, &positions, &mut cache, &cross).unwrap() };
            close_stack("decoder logits", &got, &want.data, ty.is_some());
            assert_eq!(cache.n_past, reference.n_past, "position belongs to the caller");
            for i in 0..2 {
                close_stack("self K", &cache.k[i], &reference.k[i], ty.is_some());
                close_stack("self V", &cache.v[i], &reference.v[i], ty.is_some());
            }
            decoder::advance_kv_cache(&mut cache, tokens.len());
            decoder::advance_kv_cache(&mut reference, tokens.len());
        }
        // Same allocations, new session: no stale prefix or cross cache.
        cache.clear();
        reference.clear();
        let want = cpu(|| decoder::decode(&model, &[2], &[0], &mut reference, &cross, None));
        let got = try_decode(&model, &[2], &[0], &mut cache, &cross).unwrap();
        close_stack("reset decoder", &got, &want.data, ty.is_some());
        let lengths: Vec<_> = cache.k.iter().map(Vec::len).collect();
        assert!(try_decode(&model, &[-1], &[0], &mut cache, &cross).is_none());
        assert_eq!(lengths, cache.k.iter().map(Vec::len).collect::<Vec<_>>());
        assert!(is_available(), "unsupported input must not disable Vulkan");
    }
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn failed_decode_preserves_cpu_fallback_prefix() {
    let _gpu = gpu();
    let model = model(41);
    let cross: Vec<_> = (0..2).map(|i| (tensor(&[37, 128], i + 1), tensor(&[37, 128], i + 7))).collect();
    let mut cache = KvCache::new(2);
    let _ = try_decode(&model, &[3, 4], &[0, 1], &mut cache, &cross).unwrap();
    decoder::advance_kv_cache(&mut cache, 2);
    let k = cache.k.clone();
    let v = cache.v.clone();
    let mut invalid_cross = cross.clone();
    invalid_cross[1].1.data.pop();
    assert!(try_decode(&model, &[5], &[2], &mut cache, &invalid_cross).is_none());
    assert_eq!(cache.k, k);
    assert_eq!(cache.v, v);
    assert_eq!(cache.n_past, 2);
    assert_eq!(accel::backend_name(), "cpu");
    let mut reference = KvCache::new(2);
    reference.k = k;
    reference.v = v;
    reference.n_past = 2;
    let want = cpu(|| decoder::decode(&model, &[5], &[2], &mut reference, &cross, None));
    let got = decoder::decode(&model, &[5], &[2], &mut cache, &cross, None);
    close("failed Vulkan decode resumes on CPU", &got.data, &want.data, 1e-6);
    assert_eq!(cache.k, reference.k);
    assert_eq!(cache.v, reference.v);
}

#[test]
#[ignore = "requires a real Vulkan GPU"]
fn int8_encoder_weights_keep_float_gemv_and_decoder() {
    let _gpu = gpu();
    let model = model(57);
    run("int8 encoder scope", |ctx| {
        let op = &mut ctx.ops;
        let weights = ModelWeights::new(&op.device, &model, true)?;
        for layer in &weights.enc {
            for linear in [&layer.attn.q, &layer.attn.k, &layer.attn.v, &layer.attn.out, &layer.ffn.up, &layer.ffn.down] {
                assert!(linear.w.int8.is_some());
            }
        }
        for layer in &weights.dec {
            for linear in [&layer.self_attn.q, &layer.self_attn.k, &layer.self_attn.v, &layer.self_attn.out,
                &layer.cross.q, &layer.cross.k, &layer.cross.v, &layer.cross.out, &layer.ffn.up, &layer.ffn.down] {
                assert!(linear.w.int8.is_none());
            }
        }
        assert!(weights.conv1.w.int8.is_none() && weights.conv2.w.int8.is_none() && weights.logits.int8.is_none());
        for rows in [1, 4] {
            let input = tensor(&[rows, 128], 19);
            let want = cpu(|| Tensor::linear_raw(&input, &model.encoder_layers[0].attn_q_w, &model.encoder_layers[0].attn_q_b));
            let mut plan = Layout::default();
            let x = plan.take(input.data.len())?;
            let dst = plan.take(input.data.len())?;
            op.reserve(&plan)?;
            op.write(x, &input.data)?;
            op.begin()?;
            op.linear(x, dst, rows, &weights.enc[0].attn.q)?;
            op.finish()?;
            close("int8 weights retain float GEMV", &op.read(dst)?, &want.data, 3e-5);
        }
        Ok(())
    }).unwrap();
}
