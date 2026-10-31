//! Real Vulkan checks. Comparisons are opt-in; unavailable hardware is a failure.
//! Worker churn runs by default and skips only when no device is available.

use super::*;
use crate::kokoro::{self, ops, weights::Weights as HostWeights};
use std::sync::atomic::Ordering;

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
    if let Err(e) = makepad_ai_vulkan::Device::shared() {
        eprintln!("Kokoro Vulkan worker churn skipped: no usable device ({e})");
        return;
    }
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
#[ignore = "requires a real Vulkan device"]
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
#[ignore = "requires a real Vulkan device"]
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

fn model_path(name: &str) -> String {
    std::path::Path::new(&std::env::var("MAKEPAD_KOKORO_TEST_DATA").expect("set MAKEPAD_KOKORO_TEST_DATA to the model directory"))
        .join(name).to_string_lossy().into_owned()
}

#[test]
#[ignore = "requires a real Vulkan device"]
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

#[test]
#[ignore = "requires Kokoro weights and a real Vulkan device"]
fn model_blocks_and_resident_weights() {
    let _gpu = gpu();
    let weights = HostWeights::load(&model_path("kokoro-v1_0.mktts")).unwrap();
    let voice = HostWeights::load(&model_path("af_heart.mkvoice")).unwrap();
    let style = &voice.get("style").unwrap()[10 * 256..10 * 256 + 128];
    let decoder = Decoder::load(&weights).unwrap();
    let x = mat(514, 7, 31);
    let expected = cpu(|| decoder.encode.forward(&x, style));
    for _ in 0..2 {
        close("resident residual", &adain_block(&decoder.encode, &x, style).unwrap().data, &expected.data, 3e-4);
    }
    let bert = kokoro::bert::Bert::load(&weights).unwrap();
    let text = kokoro::text_encoder::TextEncoder::load(&weights).unwrap();
    let tokens = crate::g2p::tokens("Hello there.");
    let want = cpu(|| bert.trace(&tokens));
    let got = bert.trace(&tokens);
    for (i, (a, b)) in got.layers.iter().zip(&want.layers).enumerate() { close(&format!("BERT layer {i}"), &a.data, &b.data, 5e-4); }
    close("BERT output", &got.output.data, &want.output.data, 5e-4);
    let want = cpu(|| text.trace(&tokens));
    let got = text.trace(&tokens);
    for ((label, a), (_, b)) in got.steps.iter().zip(&want.steps) { close(label, &a.data, &b.data, 2e-4); }
    close("text LSTM", &got.output.data, &want.output.data, 5e-4);

    let asr = mat(512, 9, 37);
    let f0 = Mat::from_vec(1, 18, (0..18).map(|i| 120.0 + i as f32).collect());
    let noise = mat(1, 18, 41);
    kokoro::generator::NOISE_STREAM.store(101, Ordering::Relaxed);
    let want = cpu(|| decoder.run(&asr, &f0, &noise, style));
    kokoro::generator::NOISE_STREAM.store(101, Ordering::Relaxed);
    let got = decode(&decoder, &asr, &f0, &noise, style, true).unwrap();
    close("decoder encode", &got.encode.data, &want.encode.data, 8e-4);
    for (i, (a, b)) in got.decode.iter().zip(&want.decode).enumerate() { close(&format!("decode block {i}"), &a.data, &b.data, 1e-3); }
    close("identical harmonic source", &got.generator.har_source, &want.generator.har_source, 0.0);
    for (i, (a, b)) in got.generator.noise_convs.iter().zip(&want.generator.noise_convs).enumerate() { close(&format!("noise convolution {i}"), &a.data, &b.data, 3e-4); }
    for (i, (a, b)) in got.generator.ups.iter().zip(&want.generator.ups).enumerate() { close(&format!("upsample {i}"), &a.data, &b.data, 8e-4); }
    for (i, (a, b)) in got.generator.merged.iter().zip(&want.generator.merged).enumerate() { close(&format!("generator stage {i}"), &a.data, &b.data, 2e-3); }
    close("generator spectrum", &got.generator.post.data, &want.generator.post.data, 3e-3);
    let (_, snr) = diff("decoder waveform", &got.generator.waveform, &want.generator.waveform);
    assert!(snr > 70.0, "decoder waveform SNR {snr}");
    kokoro::generator::NOISE_STREAM.store(101, Ordering::Relaxed);
    let fast = decode(&decoder, &asr, &f0, &noise, style, false).unwrap();
    close("trace versus synthesis liveness", &fast.generator.waveform, &got.generator.waveform, 0.0);
    assert_eq!(accel::backend_name(), "vulkan");
    // Dropped identities cannot retain or match recycled allocations.
    drop(decoder); drop(bert); drop(text);
    run("cache cleanup test", |ctx| {
        let _g = Graph::new(ctx, None);
        Ok(())
    }).unwrap();
    RUNTIME.with(|r| { if let Runtime::Ready(ctx) = &*r.borrow() { assert!(ctx.weights.is_empty()); } });
}

#[test]
#[ignore = "requires Kokoro weights and a real Vulkan device; runs CPU synthesis too"]
fn english_synthesis_and_real_time_factor() {
    let _gpu = gpu();
    let model = model_path("kokoro-v1_0.mktts");
    for (voice, sentences) in [
        ("af_heart.mkvoice", &[
            "Hello there.",
            "The next train leaves in five minutes.",
            "Please turn left at the next street, then continue straight ahead.",
        ][..]),
        ("bm_daniel.mkvoice", &["Could you help me find the station?"][..]),
    ] {
        let mut speaker = kokoro::KokoroSpeaker::load_with_voice(&model, &model_path(voice)).unwrap();
        for (index, sentence) in sentences.iter().enumerate() {
            kokoro::generator::NOISE_STREAM.store(2027, Ordering::Relaxed);
            let start = std::time::Instant::now();
            let expected = cpu(|| speaker.synthesize(sentence)).unwrap();
            let cpu_secs = start.elapsed().as_secs_f64();
            kokoro::generator::NOISE_STREAM.store(2027, Ordering::Relaxed);
            let start = std::time::Instant::now();
            let actual = speaker.synthesize(sentence).unwrap();
            let vulkan_secs = start.elapsed().as_secs_f64();
            assert_eq!(accel::backend_name(), "vulkan", "CPU fallback invalidates timing");
            let (max, snr) = diff(sentence, &actual.samples, &expected.samples);
            assert_eq!(actual.sample_rate, 24000);
            assert_eq!(actual.timings, expected.timings, "word durations");
            assert_eq!(actual.phones, expected.phones, "phoneme durations");
            assert_eq!(actual.samples.len() % 600, 0, "alignment frames are 600 samples");
            assert!(actual.samples.iter().any(|v| v.abs() > 0.01), "silent waveform");
            assert!(actual.samples.iter().all(|v| v.abs() <= 1.0), "unscaled waveform");
            assert!(snr > 30.0, "full synthesis waveform SNR {snr}");
            let duration = actual.samples.len() as f64 / actual.sample_rate as f64;
            eprintln!("SYNTH voice={voice} index={index} samples={} duration={duration:.3} cpu_s={cpu_secs:.4} vulkan_s={vulkan_secs:.4} cpu_rtf={:.4} vulkan_rtf={:.4} max_abs={max:.6} snr_db={snr:.3}", actual.samples.len(), cpu_secs / duration, vulkan_secs / duration);
            // Repeat with resident weights and identical excitation noise.
            kokoro::generator::NOISE_STREAM.store(2027, Ordering::Relaxed);
            let start = std::time::Instant::now();
            let warm = speaker.synthesize(sentence).unwrap();
            let warm_secs = start.elapsed().as_secs_f64();
            close("warm determinism", &warm.samples, &actual.samples, 0.0);
            eprintln!("WARM voice={voice} index={index} seconds={warm_secs:.4} rtf={:.4}", warm_secs / duration);
        }
    }
}
