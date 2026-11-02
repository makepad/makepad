//! Real-device checks against self-contained CPU references.
//! A working Vulkan device is required.

use super::*;
use crate::kokoro::ops;

static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Gpu {
    _serial: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Gpu {
    fn drop(&mut self) {
        RUNTIME.with(|r| *r.borrow_mut() = Runtime::New);
        TEST_VULKAN.with(|v| v.set(false));
    }
}

fn gpu() -> Gpu {
    let guard = Gpu { _serial: GPU.lock().unwrap_or_else(|e| e.into_inner()) };
    TEST_VULKAN.with(|v| v.set(true));
    assert!(available(), "a working Vulkan device is required");
    guard
}

#[test]
fn runtime_survives_100_worker_threads() {
    makepad_ai_vulkan::Device::shared().expect("a working Vulkan device is required");
    for i in 0..100 {
        std::thread::spawn(move || {
            TEST_VULKAN.with(|v| v.set(true));
            assert!(available(), "Kokoro Vulkan runtime {i} failed to initialize");
            // Let thread exit drop the runtime's Ops and all its resources.
        }).join().unwrap_or_else(|_| panic!("Kokoro Vulkan worker {i} panicked"));
    }
}

fn cpu<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset { fn drop(&mut self) { accel::TEST_CPU.with(|v| v.set(self.0)); } }
    let _reset = Reset(accel::TEST_CPU.with(|v| v.replace(true)));
    f()
}

fn values(n: usize, seed: u32) -> Vec<f32> {
    let mut state = seed;
    (0..n).map(|_| {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        (state >> 8) as f32 / 16777216.0 - 0.5
    }).collect()
}

fn mat(rows: usize, cols: usize, seed: u32) -> Mat { Mat::from_vec(rows, cols, values(rows * cols, seed)) }

fn diff(label: &str, actual: &[f32], expected: &[f32]) -> (f64, f64) {
    assert_eq!(actual.len(), expected.len(), "{label}: sample count");
    let mut error = 0.0f64;
    let mut signal = 0.0f64;
    let mut max = 0.0f64;
    for (&a, &e) in actual.iter().zip(expected) {
        assert!(a.is_finite() && e.is_finite(), "{label}: nonfinite value");
        let d = a as f64 - e as f64;
        max = max.max(d.abs());
        error += d * d;
        signal += (e as f64) * (e as f64);
    }
    let snr = 10.0 * (signal / error.max(1e-300)).log10();
    eprintln!("{label}: n={} max_abs={max:.8} rms_error={:.8} snr_db={snr:.3}", actual.len(), (error / actual.len().max(1) as f64).sqrt());
    (max, snr)
}

fn close(label: &str, actual: &[f32], expected: &[f32], tolerance: f32) {
    diff(label, actual, expected);
    for (i, (&a, &e)) in actual.iter().zip(expected).enumerate() {
        assert!((a - e).abs() <= tolerance * (1.0 + e.abs()), "{label}[{i}]: {a} vs {e}");
    }
}

#[test]
fn matrices_and_convolutions() {
    let _gpu = gpu();
    for (m, k, n) in [(1, 1, 1), (3, 7, 5), (33, 35, 37), (64, 128, 65), (1, 128, 16), (5, 128, 16)] {
        let a = mat(m, k, 1);
        let b = mat(k, n, 2);
        let mut expected = Mat::zeros(m, n);
        for r in 0..m { for c in 0..n { for j in 0..k { expected.data[r * n + c] += a.at(r, j) * b.at(j, c); } } }
        close("GEMM NN", &matmul(&a.data, &b.data, m, k, n, false).unwrap(), &expected.data, 2e-5);
        close("GEMM NT", &matmul(&a.data, &b.transpose().data, m, k, n, true).unwrap(), &expected.data, 2e-5);
        let bias = values(n, 3);
        let weights = b.transpose().data;
        let want = cpu(|| ops::linear(&a, &weights, &bias, n));
        let actual = run("linear test", |ctx| {
            let mut g = Graph::new(ctx, None);
            let x = g.input(&a)?;
            let y = g.linear(x, &weights, &bias)?;
            Ok(g.finish(&[y])?.remove(0))
        }).unwrap();
        close("linear bias", &actual.data, &want.data, 2e-5);
    }
    for (ic, time, oc, kernel, stride, dilation, pad) in [
        (1, 1, 1, 1, 1, 1, 0), (3, 35, 7, 3, 1, 1, 1),
        (8, 37, 33, 5, 2, 1, 2), (5, 67, 9, 7, 1, 3, 9), (3, 17, 5, 3, 2, 2, 2),
    ] {
        let x = mat(ic, time, 11);
        let w = values(ic * oc * kernel, 12);
        let b = values(oc, 13);
        let want = cpu(|| ops::conv1d_general(&x, &w, &b, oc, pad, stride, dilation));
        let got = conv(&x, &w, &b, oc, pad, stride, dilation, false).unwrap();
        close("conv1d", &got.data, &want.data, 2e-5);
        if dilation == 1 {
            let want = cpu(|| ops::conv_transpose1d(&x, &w, &b, oc, stride, pad));
            let got = conv(&x, &w, &b, oc, pad, stride, 1, true).unwrap();
            close("conv transpose", &got.data, &want.data, 2e-5);
        }
    }
    // Actual upsampling strides, with partial tiles on both axes.
    for (stride, kernel, pad) in [(10, 20, 5), (6, 12, 3)] {
        let x = mat(7, 13, 19);
        let w = values(7 * 5 * kernel, 23);
        let b = values(5, 29);
        let want = cpu(|| ops::conv_transpose1d(&x, &w, &b, 5, stride, pad));
        close("generator upsample", &conv(&x, &w, &b, 5, pad, stride, 1, true).unwrap().data, &want.data, 3e-5);
    }
}

#[test]
fn norms_activations_and_layouts() {
    let _gpu = gpu();
    for cols in [1, 3, 37, 513, 4097] {
        let x = mat(7, cols, 1);
        let a = values(7, 3).into_iter().map(|v| v + 1.0).collect::<Vec<_>>();
        let affine = mat(1, 14, 5);
        let gamma = values(cols, 7);
        let beta = values(cols, 9);
        let w = values(7 * 3, 11);
        let bias = values(7, 13);
        let got = run("norms test", |ctx| {
            let mut g = Graph::new(ctx, None);
            let input = g.input(&x)?;
            let af = g.input(&affine)?;
            let norm = g.norm(input, None, None, 1e-5, 0, 0.0, None)?;
            let ada = g.norm(input, None, Some(af), 1e-5, 0, 0.0, None)?;
            let leaky = g.leaky(input, 0.2)?;
            let snake = g.element(input, None, 3, 1.0, Some(&a))?;
            let gelu = g.element(input, None, 4, 1.0, None)?;
            let added = g.add(input, leaky, std::f32::consts::FRAC_1_SQRT_2)?;
            let norm_rows = g.norm(input, Some((&gamma, &beta)), None, 1e-5, 0, 0.0, None)?;
            let fused_snake = g.norm(input, None, Some(af), 1e-5, 2, 0.2, Some(&a))?;
            let fused_leaky = g.norm(input, None, Some(af), 1e-5, 1, 0.2, None)?;
            let transposed = g.transpose(input)?;
            let repeated = g.repeat(input, 2)?;
            let concatenated = g.concat(&[input, leaky])?;
            let depthwise = g.depthwise(input, &w, &bias, 2, 1, 1)?;
            let mut outputs = vec![norm, ada, leaky, snake, gelu, added, norm_rows, fused_snake, fused_leaky, transposed, repeated, concatenated, depthwise];
            if cols > 1 { outputs.push(g.reflect(input, 1)?); }
            g.finish(&outputs)
        }).unwrap();
        let norm = cpu(|| ops::instance_norm_time(&x, 1e-5));
        let mut ada = norm.clone();
        for c in 0..7 { for t in 0..cols { ada.data[c * cols + t] = norm.at(c, t) * (1.0 + affine.data[c]) + affine.data[7 + c]; } }
        let mut leaky = x.clone(); ops::leaky_relu(&mut leaky, 0.2);
        let mut snake = x.clone(); ops::snake(&mut snake, &a);
        let gelu = x.data.iter().copied().map(ops::gelu_new).collect::<Vec<_>>();
        let add = x.data.iter().zip(&leaky.data).map(|(a, b)| (a + b) * std::f32::consts::FRAC_1_SQRT_2).collect::<Vec<_>>();
        let norm_rows = ops::layer_norm_rows(&x, &gamma, &beta, 1e-5);
        let mut fused_snake = ada.clone(); ops::snake(&mut fused_snake, &a);
        let mut fused_leaky = ada.clone(); ops::leaky_relu(&mut fused_leaky, 0.2);
        for (i, want) in [&norm.data, &ada.data, &leaky.data, &snake.data, &gelu, &add, &norm_rows.data, &fused_snake.data, &fused_leaky.data,
            &x.transpose().data, &ops::nearest_repeat(&x, 2).data, &ops::concat_rows(&[&x, &leaky]).data,
            &ops::conv_transpose1d_depthwise(&x, &w, &bias, 2, 1, 1).data].iter().enumerate() {
            close(&format!("element {i}, cols={cols}"), &got[i].data, want, 2e-5);
        }
        if cols > 1 { close("reflect", &got[13].data, &ops::reflect_pad(&x, 1, 0).data, 0.0); }
    }
}

#[test]
fn recurrent_directions() {
    let _gpu = gpu();
    for (input, hidden, time) in [(7, 8, 1), (13, 12, 17), (512, 256, 37), (640, 256, 121)] {
        let layer = |seed| ops::LstmLayer {
            input, hidden,
            weight_ih: values(4 * hidden * input, seed).into_iter().map(|v| v * 0.1).collect::<Vec<f32>>().into(),
            weight_hh: values(4 * hidden * hidden, seed + 1).into_iter().map(|v| v * 0.1).collect::<Vec<f32>>().into(),
            bias_ih: values(4 * hidden, seed + 2).into(),
            bias_hh: values(4 * hidden, seed + 3).into(),
        };
        let model = ops::BiLstm { forward: layer(31), reverse: layer(41) };
        let owner = std::sync::Arc::new(());
        let x = mat(time, input, 71);
        let want = cpu(|| model.run(&x));
        let got = bilstm(&owner, &model, &x).unwrap();
        close("bidirectional LSTM", &got.data, &want.data, 5e-6);
    }
}
