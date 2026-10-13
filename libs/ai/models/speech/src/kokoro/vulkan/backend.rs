//! Kokoro's Vulkan arm. Weights stay resident under live model identities;
//! blocks record into a liveness-planned arena and submit once. Spectral
//! transforms remain on the synthesis worker's CPU. Each recurrent direction
//! is one workgroup, with its state in shared memory across all timesteps.

#[path = "graph.rs"]
mod graph;
#[cfg(test)]
#[path = "tests.rs"]
mod tests;

use crate::kokoro::{accel, adain::{AdaIn1d, AdainResBlk1d}, decoder::{Decoder, DecoderTrace},
    generator::{AdaResBlock1, Conv, Generator, GeneratorTrace}, ops::{Mat, nearest_repeat}, stft};
use graph::{Graph, Ops, Tensor, Weights};
use makepad_ai_vulkan::Result;
use std::cell::RefCell;
use std::collections::HashMap;

enum Runtime { New, Ready(Box<Context>), Failed }

thread_local! {
    static RUNTIME: RefCell<Runtime> = const { RefCell::new(Runtime::New) };
    #[cfg(test)]
    static TEST_VULKAN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn selected() -> bool {
    if accel::cpu_forced() { return false; }
    #[cfg(test)]
    if TEST_VULKAN.with(|v| v.get()) { return true; }
    match std::env::var("MAKEPAD_KOKORO_BACKEND").as_deref() {
        Ok("vulkan") => true,
        Ok(_) => false,
        Err(_) => cfg!(target_os = "android"),
    }
}

struct Context {
    weights: HashMap<usize, Weights>,
    submissions: usize,
    dispatches: usize,
    peak_arena: u64,
    // Resources precede the shared device reference in Ops.
    ops: Ops,
}

fn run<T>(label: &'static str, f: impl FnOnce(&mut Context) -> Result<T>) -> Option<T> {
    if !selected() { return None; }
    RUNTIME.with(|runtime| {
        let mut runtime = runtime.try_borrow_mut().ok()?;
        if matches!(*runtime, Runtime::New) {
            match Ops::new() {
                Ok(ops) => *runtime = Runtime::Ready(Box::new(Context {
                    weights: HashMap::new(), submissions: 0, dispatches: 0, peak_arena: 0, ops,
                })),
                Err(err) => {
                    eprintln!("[kokoro] Vulkan unavailable: {err}; using CPU");
                    *runtime = Runtime::Failed;
                }
            }
        }
        let Runtime::Ready(ctx) = &mut *runtime else { return None };
        let _profile = super::ops::op_timer(label);
        match f(ctx) {
            Ok(value) => Some(value),
            Err(err) => {
                eprintln!("[kokoro] Vulkan disabled after {label}: {err}; using CPU");
                *runtime = Runtime::Failed;
                None
            }
        }
    })
}

/// True once this thread's Vulkan runtime holds uploaded Kokoro weights (the
/// host copies are then only a CPU fallback).
pub(crate) fn weights_resident() -> bool {
    RUNTIME.with(|runtime| match runtime.try_borrow() {
        Ok(runtime) => matches!(&*runtime, Runtime::Ready(ctx) if ctx.weights.values().any(|w| w.bytes() > 0)),
        Err(_) => false,
    })
}

pub(crate) fn available() -> bool { run("initialize", |_| Ok(())).is_some() }

pub(crate) fn bilstm(owner: &std::sync::Arc<()>, model: &crate::kokoro::ops::BiLstm, x: &Mat) -> Option<Mat> {
    // This shared-memory kernel supports the model's 256-wide directions
    // and smaller multiples of four. Other architectures keep the reference.
    for layer in [&model.forward, &model.reverse] {
        if layer.hidden == 0 || layer.hidden > 256 || layer.hidden % 4 != 0 || layer.input != x.cols { return None; }
    }
    run("bilstm_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(owner));
        let x = g.input(x)?;
        let x = g.transpose(x)?;
        let forward = g.lstm(x, &model.forward, false)?;
        let reverse = g.lstm(x, &model.reverse, true)?;
        let out = g.concat(&[forward, reverse])?;
        let out = g.transpose(out)?;
        Ok(g.finish(&[out])?.remove(0))
    })
}

pub(crate) fn matmul(a: &[f32], b: &[f32], m: usize, k: usize, n: usize, transposed: bool) -> Option<Vec<f32>> {
    run("matmul_vulkan", |ctx| {
        if a.len() != m * k || b.len() != k * n { return Err("Kokoro matrix storage mismatch".into()); }
        let input = if transposed { Mat::from_vec(n, k, b.to_vec()).transpose() } else { Mat::from_vec(k, n, b.to_vec()) };
        let mut g = Graph::new(ctx, None);
        let x = g.input(&input)?;
        let out = g.matmul(x, a, m, None)?;
        Ok(g.finish(&[out])?.remove(0).data)
    })
}

pub(crate) fn conv(x: &Mat, weight: &[f32], bias: &[f32], channels: usize, pad: usize, stride: usize, dilation: usize, transposed: bool) -> Option<Mat> {
    run("convolution_vulkan", |ctx| {
        let mut g = Graph::new(ctx, None);
        let x = g.input(x)?;
        let out = g.conv(x, weight, Some(bias), channels, pad, stride, dilation, transposed)?;
        Ok(g.finish(&[out])?.remove(0))
    })
}

impl Graph<'_> {
    fn adain(&mut self, norm: &AdaIn1d, x: Tensor, style: Tensor, activation: u32, alpha: Option<&[f32]>) -> Result<Tensor> {
        if x.rows != norm.channels || style.rows != 1 || style.cols != 128 { return Err("Kokoro style dimensions disagree".into()); }
        let affine = self.linear(style, &norm.fc_weight, &norm.fc_bias)?;
        self.norm(x, None, Some(affine), 1e-5, activation, 0.2, alpha)
    }

    fn residual(&mut self, block: &AdainResBlk1d, x: Tensor, style: Tensor) -> Result<Tensor> {
        let mut h = self.adain(&block.norm1, x, style, 1, None)?;
        if let Some((w, b)) = &block.pool { h = self.depthwise(h, w, b, 2, 1, 1)?; }
        h = self.conv(h, &block.conv1.0, Some(&block.conv1.1), block.dim_out, 1, 1, 1, false)?;
        h = self.adain(&block.norm2, h, style, 1, None)?;
        h = self.conv(h, &block.conv2.0, Some(&block.conv2.1), block.dim_out, 1, 1, 1, false)?;
        let mut shortcut = if block.upsample { self.repeat(x, 2)? } else { x };
        if let Some(w) = &block.conv1x1 { shortcut = self.conv(shortcut, w, None, block.dim_out, 0, 1, 1, false)?; }
        self.add(h, shortcut, std::f32::consts::FRAC_1_SQRT_2)
    }

    fn convolution(&mut self, conv: &Conv, x: Tensor) -> Result<Tensor> {
        self.conv(x, &conv.weight, Some(&conv.bias), conv.out_channels, conv.pad, conv.stride, conv.dilation, false)
    }

    fn source_residual(&mut self, block: &AdaResBlock1, mut x: Tensor, style: Tensor) -> Result<Tensor> {
        for i in 0..block.convs1.len() {
            let h = self.adain(&block.adain1[i], x, style, 2, Some(&block.alpha1[i]))?;
            let h = self.convolution(&block.convs1[i], h)?;
            let h = self.adain(&block.adain2[i], h, style, 2, Some(&block.alpha2[i]))?;
            let h = self.convolution(&block.convs2[i], h)?;
            x = self.add(h, x, 1.0)?;
        }
        Ok(x)
    }

    // Checkpoints are returned in the same order by generate/decode. They are
    // kept alive only for explicit trace requests, never normal synthesis.
    fn generator(&mut self, model: &Generator, mut x: Tensor, style: Tensor, har: Tensor) -> Result<Vec<Tensor>> {
        // The decoder exposes its generator for replacement. Its weight
        // lifetime must therefore be independent of the decoder's identity.
        self.set_owner(&model.vulkan_id);
        let mut noise_convs = Vec::new();
        let mut ups = Vec::new();
        let mut merged = Vec::new();
        let mut reflect = x;
        for stage in 0..model.ups.len() {
            x = self.leaky(x, 0.1)?;
            let projected = self.convolution(&model.noise_convs[stage], har)?;
            noise_convs.push(projected);
            let source = self.source_residual(&model.noise_res[stage], projected, style)?;
            let up = &model.ups[stage];
            let mut y = self.conv(x, &up.weight, Some(&up.bias), up.out_channels, up.pad, up.stride, 1, true)?;
            ups.push(y);
            if stage == model.ups.len() - 1 { y = self.reflect(y, 1)?; reflect = y; }
            y = self.add(y, source, 1.0)?;
            let mut sum = self.source_residual(&model.resblocks[stage * 3], y, style)?;
            for i in 1..3 {
                let h = self.source_residual(&model.resblocks[stage * 3 + i], y, style)?;
                sum = self.add(sum, h, 1.0)?;
            }
            x = self.element(sum, None, 0, 1.0 / 3.0, None)?;
            merged.push(x);
        }
        x = self.leaky(x, 0.01)?;
        let post = self.convolution(&model.conv_post, x)?;
        let mut out = noise_convs;
        out.extend(ups);
        out.push(reflect);
        out.extend(merged);
        out.push(post);
        Ok(out)
    }
}

pub(crate) fn adain_block(block: &AdainResBlk1d, x: &Mat, style: &[f32]) -> Option<Mat> {
    run("adain_block_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(&block.vulkan_id));
        let x = g.input(x)?;
        let s = g.input(&Mat::from_vec(1, style.len(), style.to_vec()))?;
        let out = g.residual(block, x, s)?;
        Ok(g.finish(&[out])?.remove(0))
    })
}

fn finish_generator(har_source: Vec<f32>, har: Mat, values: Vec<Mat>, trace: bool) -> GeneratorTrace {
    let mut values = values.into_iter();
    let noise_convs = if trace { values.by_ref().take(2).collect() } else { Vec::new() };
    let ups = if trace { values.by_ref().take(2).collect() } else { Vec::new() };
    let reflect = if trace { values.next().unwrap() } else { Mat::zeros(0, 0) };
    let merged = if trace { values.by_ref().take(2).collect() } else { Vec::new() };
    let post = values.next().unwrap();
    let mut re_im = Mat::zeros(post.rows, post.cols);
    for bin in 0..stft::BINS {
        for t in 0..post.cols {
            let magnitude = post.at(bin, t).exp();
            let phase = post.at(stft::BINS + bin, t).sin();
            re_im.data[bin * post.cols + t] = magnitude * phase.cos();
            re_im.data[(stft::BINS + bin) * post.cols + t] = magnitude * phase.sin();
        }
    }
    let waveform = stft::istft(&re_im);
    GeneratorTrace { har_source, har, noise_convs, ups, reflect, merged, post, waveform }
}

pub(crate) fn generate(model: &Generator, x: &Mat, style: &[f32], har_source: &[f32], har: &Mat) -> Option<GeneratorTrace> {
    run("generator_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(&model.vulkan_id));
        let x = g.input(x)?;
        let s = g.input(&Mat::from_vec(1, style.len(), style.to_vec()))?;
        let h = g.input(har)?;
        let out = g.generator(model, x, s, h)?;
        let values = g.finish(&out)?;
        Ok(finish_generator(har_source.to_vec(), har.clone(), values, true))
    })
}

pub(crate) fn decode(model: &Decoder, asr: &Mat, f0: &Mat, noise: &Mat, style: &[f32], trace: bool) -> Option<DecoderTrace> {
    run("decoder_generator_vulkan", |ctx| {
        let har_source = model.generator.source(&nearest_repeat(f0, 300));
        let har = stft::stft_mag_phase(&har_source);
        let mut g = Graph::new(ctx, Some(&model.vulkan_id));
        let asr = g.input(asr)?;
        let f0 = g.input(f0)?;
        let noise = g.input(noise)?;
        let s = g.input(&Mat::from_vec(1, style.len(), style.to_vec()))?;
        let h = g.input(&har)?;
        let f0 = g.convolution(&model.f0_conv, f0)?;
        let noise = g.convolution(&model.n_conv, noise)?;
        let asr_res = g.convolution(&model.asr_res, asr)?;
        let combined = g.concat(&[asr, f0, noise])?;
        let mut x = g.residual(&model.encode, combined, s)?;
        let mut checkpoints = vec![f0, noise, asr_res, x];
        for block in &model.decode {
            let combined = g.concat(&[x, asr_res, f0, noise])?;
            x = g.residual(block, combined, s)?;
            checkpoints.push(x);
        }
        let generator = g.generator(&model.generator, x, s, h)?;
        let values = if trace {
            checkpoints.extend(generator);
            g.finish(&checkpoints)?
        } else { g.finish(&[*generator.last().unwrap()])? };
        let mut values = values.into_iter();
        let mut next = || if trace { values.next().unwrap() } else { Mat::zeros(0, 0) };
        let (f0, noise, asr_res, encode) = (next(), next(), next(), next());
        let decode = if trace { values.by_ref().take(model.decode.len()).collect() } else { Vec::new() };
        let generator = finish_generator(har_source, har, values.collect(), trace);
        if std::env::var_os("KOKORO_PROFILE").is_some() {
            let weight_bytes: u64 = ctx.weights.values().map(Weights::bytes).sum();
            eprintln!("[kokoro Vulkan] submissions={} dispatches={} arena_peak={:.2} MiB weights={:.2} MiB", ctx.submissions, ctx.dispatches, ctx.peak_arena as f64 / 1048576.0, weight_bytes as f64 / 1048576.0);
        }
        Ok(DecoderTrace { f0, noise, asr_res, encode, decode, generator })
    })
}

pub(crate) fn bert(model: &crate::kokoro::bert::Bert, embedded: &Mat, layers: usize) -> Option<crate::kokoro::bert::Trace> {
    run("bert_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(&model.vulkan_id));
        let x = g.input(embedded)?;
        let mut x = g.linear(x, &model.mapping.0, &model.mapping.1)?;
        let mut checkpoints = Vec::new();
        let l = &model.layer;
        for _ in 0..layers {
            let q = g.linear(x, &l.query.0, &l.query.1)?;
            let k = g.linear(x, &l.key.0, &l.key.1)?;
            let v = g.linear(x, &l.value.0, &l.value.1)?;
            let a = g.attention(q, k, v, 12)?;
            let a = g.linear(a, &l.dense.0, &l.dense.1)?;
            let a = g.add(a, x, 1.0)?;
            let a = g.norm(a, Some((&l.attention_norm.0, &l.attention_norm.1)), None, 1e-12, 0, 0.0, None)?;
            let h = g.linear(a, &l.ffn.0, &l.ffn.1)?;
            let h = g.element(h, None, 4, 1.0, None)?;
            let h = g.linear(h, &l.ffn_output.0, &l.ffn_output.1)?;
            let h = g.add(h, a, 1.0)?;
            x = g.norm(h, Some((&l.full_norm.0, &l.full_norm.1)), None, 1e-12, 0, 0.0, None)?;
            checkpoints.push(x);
        }
        checkpoints.push(g.linear(x, &model.encoder.0, &model.encoder.1)?);
        let mut values = g.finish(&checkpoints)?;
        let output = values.pop().unwrap();
        Ok(crate::kokoro::bert::Trace { layers: values, output })
    })
}

pub(crate) fn text_convs(model: &crate::kokoro::text_encoder::TextEncoder, input: &Mat) -> Option<Vec<(String, Mat)>> {
    run("text_convs_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(&model.vulkan_id));
        let mut x = g.input(input)?;
        let mut outputs = Vec::new();
        let mut names = Vec::new();
        for (i, b) in model.blocks.iter().enumerate() {
            x = g.conv(x, &b.weight, Some(&b.bias), b.bias.len(), 2, 1, 1, false)?;
            outputs.push(x); names.push(format!("conv{i}"));
            let t = g.transpose(x)?;
            let t = g.norm(t, Some((&b.gamma, &b.beta)), None, 1e-5, 0, 0.0, None)?;
            x = g.transpose(t)?;
            outputs.push(x); names.push(format!("norm{i}"));
            x = g.leaky(x, 0.2)?;
            outputs.push(x); names.push(format!("act{i}"));
        }
        Ok(names.into_iter().zip(g.finish(&outputs)?).collect())
    })
}

pub(crate) fn prosody(model: &crate::kokoro::predictor::Predictor, input: &Mat, style: &[f32]) -> Option<(Vec<Mat>, Vec<Mat>, Mat, Mat)> {
    run("prosody_vulkan", |ctx| {
        let mut g = Graph::new(ctx, Some(&model.vulkan_id));
        let x = g.input(input)?;
        let s = g.input(&Mat::from_vec(1, style.len(), style.to_vec()))?;
        let mut outputs = Vec::new();
        for (blocks, proj) in [(&model.f0_blocks, &model.f0_proj), (&model.noise_blocks, &model.noise_proj)] {
            let mut h = x;
            for block in blocks { h = g.residual(block, h, s)?; outputs.push(h); }
            outputs.push(g.conv(h, &proj.0, Some(&proj.1), 1, 0, 1, 1, false)?);
        }
        let mut values = g.finish(&outputs)?.into_iter();
        let f0_blocks = values.by_ref().take(model.f0_blocks.len()).collect();
        let f0 = values.next().unwrap();
        let noise_blocks = values.by_ref().take(model.noise_blocks.len()).collect();
        let noise = values.next().unwrap();
        Ok((f0_blocks, noise_blocks, f0, noise))
    })
}
