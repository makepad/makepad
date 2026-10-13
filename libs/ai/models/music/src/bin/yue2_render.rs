//! yue2_render: run the native YuE2 pipeline stage by stage, and check it
//! against reference dumps (`libs/ai/hub/python/yue2_oracle_dump.py`).
//!
//!   yue2_render --model DIR --vae DIR --fixture song.json --out DIR [--max-semantic N]
//!   yue2_render --model DIR --check ORACLE_DIR
//!     [--stage tok|prefill|greedy|tf|sample-device|sample|bench|vae|nar|flow|acoustic|synth|attnbench|narlayer|all]
//!     [--ar-decode graph|host] [--bench-phase abc|semantic] [--vae DIR]
//!     [--bench-swiglu both|fused|separate]
//!   yue2_render --stage narlayer   # no model/fixture; one operation resident at a time
//!     [--narlayer-op all|attention|linears]
//! AR precision: MAKEPAD_YUE2_AR_WEIGHTS=q8|bf16 (default q8).
//! Q8 graph `--stage bench` also reports cudaEvent counts/times by kernel
//! class and shape for one event-instrumented replay, after graph timing.
//! SwiGLU variants sweep the same 28 packed weights; the switch is bench-only.
//! NAR precision: MAKEPAD_YUE2_NAR_PREC=bf16|fast (default bf16).
//!   yue2_render --model DIR --vae DIR --check ORACLE_DIR --stage acoustic [--velocity-evals 64]
//!   yue2_render --vae DIR --check ORACLE_DIR --stage vae [--vae-frames N]
//!     (--vae-frames limits output cores; up to 16 following latents supply the right halo)
//!
//! MAKEPAD_YUE2_BACKEND=graph selects ggml IR with host sampling;
//! MAKEPAD_AI_GRAPH_BACKEND=metal|cuda selects its executor. Without a
//! CUDA store ggml is automatic. --ar-decode controls only the CUDA store.
//! Graph VAE/audio stages require Metal; CUDA can run the transformer and
//! flow-latent gates through the same graphs.
//! MAKEPAD_YUE2_VAE_PROFILE=1 adds completed per-block wall/GPU timings on
//! Metal. Leave it unset for throughput (profiling waits at block boundaries).
//!
//! fixture json: {"style": "...", "lyrics": "...", "seed": 1, "cot": "full", "abc": "..."?}

use makepad_ai_common::json::Json;
use makepad_ai_common::backend::{
    gpu_bf16_round, gpu_download, gpu_graph_capture, gpu_graph_launch,
    gpu_linear_nt_cached_q8_0_f32acc_keyed, gpu_linear_nt_cached_q8_0_swiglu_keyed,
    gpu_perf_stats, gpu_profile_kernels, gpu_slice_cols, gpu_swiglu_value_gate,
    gpu_synchronize, gpu_upload,
    GpuAttentionPrecision, GpuKernelProfile, GpuLinearPart, GpuPerfStats, GpuTensor,
};
use makepad_ai_music::yue2::{
    Yue2Cot, Yue2Phase, Yue2Request, Yue2Sampling, CODEC_OFFSET, EOD, MUSIC_END,
    YUE2_FF, YUE2_HIDDEN, YUE2_LATENT_DIM, YUE2_LATENT_HOP, YUE2_LAYERS, YUE2_NAMESPACE,
};
use makepad_ai_music::yue2_lm::{Yue2ArWeights, Yue2DecodeMode, Yue2KvCache, Yue2NarChunk, Yue2NarPrecision};
use makepad_ai_music::yue2_pipeline::{torch_cpu_randn, Yue2DecodeSession, Yue2Pipeline, Yue2Progress, Yue2Vae, YUE2_VAE_HALO_FRAMES};
use makepad_ai_music::yue2_sample::{Yue2DeviceSampler, Yue2Sampler};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[path = "yue2_render/graph.rs"]
mod graph;

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn read_i32(path: &Path) -> Vec<u32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes.chunks_exact(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u32).collect()
}

fn read_f32(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn write_f32(path: &Path, data: &[f32]) {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn write_i32(path: &Path, data: &[u32]) {
    let mut bytes = Vec::with_capacity(data.len() * 4);
    for v in data {
        bytes.extend_from_slice(&(*v as i32).to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn write_wav(path: &Path, interleaved: &[f32], rate: u32) {
    let data_len = (interleaved.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for v in interleaved {
        b.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn load_request(path: &Path) -> Yue2Request {
    let text = std::fs::read_to_string(path).unwrap();
    let json = Json::parse(&text).unwrap();
    let get = |k: &str| json.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    Yue2Request {
        style: get("style").expect("style"),
        lyrics: get("lyrics").expect("lyrics"),
        cot: Yue2Cot::parse(&get("cot").unwrap_or_else(|| "full".into())).expect("cot"),
        seed: json.get("seed").and_then(|v| v.as_f64()).unwrap_or(831001.0) as u64,
        abc: get("abc"),
        cfg_scale: json.get("cfg_scale").and_then(|v| v.as_f64()).map(|v| v as f32),
    }
}

/// max |a-b|, relative L2, and whether argmax agrees.
fn compare(name: &str, ours: &[f32], refr: &[f32]) -> f64 {
    assert_eq!(ours.len(), refr.len(), "{name}: length {} vs {}", ours.len(), refr.len());
    let mut max_abs = 0.0f64;
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (&a, &b) in ours.iter().zip(refr) {
        if !b.is_finite() {
            continue;
        }
        let d = (a as f64 - b as f64).abs();
        max_abs = max_abs.max(d);
        num += d * d;
        den += (b as f64) * (b as f64);
    }
    let rel = (num / den.max(1e-30)).sqrt();
    println!("{name}: max_abs {max_abs:.4e} rel_l2 {rel:.4e}");
    rel
}

/// A bounded oracle-prefix probe must retain right context: truncating the
/// latent input exactly at N changes the decoder boundary and cannot equal
/// the full-song oracle there. Compare only N cores, including their full
/// right edge, and explicitly report the extra halo work.
fn check_vae(vae_dir: &Path, dir: &Path, limit: Option<usize>) {
    let latents = read_f32(&dir.join("latents.f32"));
    assert!(!latents.is_empty() && latents.len() % YUE2_LATENT_DIM == 0, "VAE oracle latent shape");
    let frames = latents.len() / YUE2_LATENT_DIM;
    let requested = limit.unwrap_or(frames).min(frames);
    assert!(requested > 0, "--vae-frames must be positive");
    let decode_frames = requested.saturating_add(YUE2_VAE_HALO_FRAMES).min(frames);
    let t = Instant::now();
    let vae = Yue2Vae::load(vae_dir).unwrap();
    println!("vae load {:.3}s", t.elapsed().as_secs_f64());
    println!("vae oracle prefix: {requested}/{frames} core latents, {} right-halo latents, {decode_frames} decoded latents",
        decode_frames - requested);
    let t = Instant::now();
    let audio = vae.decode_prefix(&latents[..decode_frames * YUE2_LATENT_DIM], requested, &mut |_| Ok(())).unwrap();
    println!("vae decode {decode_frames} latents {:.3}s (graph weight packing/compilation included)", t.elapsed().as_secs_f64());
    let refr = read_f32(&dir.join("audio.f32"));
    if requested < frames {
        let samples = requested * YUE2_LATENT_HOP * 2;
        assert_eq!(audio.len(), samples, "VAE prefix length");
        assert!(refr.len() >= samples, "VAE oracle prefix length");
    } else {
        assert_eq!(audio.len(), refr.len(), "VAE full-song length");
    }
    assert!(audio.iter().all(|v| v.is_finite()), "VAE nonfinite audio");
    let rel = compare("vae audio", &audio, &refr[..audio.len()]);
    let snr = -20.0 * rel.log10();
    println!("vae SNR {snr:.2} dB ({} stereo samples)", audio.len() / 2);
    assert!(snr >= 45.0, "VAE SNR gate");
}

fn argmax(v: &[f32]) -> usize {
    let mut best = 0;
    for (i, &x) in v.iter().enumerate() {
        if x > v[best] {
            best = i;
        }
    }
    best
}

fn topk_overlap(a: &[f32], b: &[f32], k: usize) -> usize {
    let top = |v: &[f32]| {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|&i, &j| v[j].partial_cmp(&v[i]).unwrap_or(std::cmp::Ordering::Equal));
        idx.truncate(k);
        idx
    };
    let ta = top(a);
    let tb = top(b);
    ta.iter().filter(|i| tb.contains(i)).count()
}

fn prefix_match(a: &[u32], b: &[u32]) -> usize {
    a.iter().zip(b).take_while(|(x, y)| x == y).count()
}

// Deterministic, zero-centred bf16-grid inputs shared by every candidate.
// Unit-variance activations and fan-in-scaled weights keep linear outputs
// near the normalized transformer range, including the 6144-wide down GEMM.
fn narlayer_random(n: usize, seed: u64, amplitude: f32) -> Vec<f32> {
    let mut state = seed;
    (0..n).map(|_| {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let value = (((state >> 40) as f32 / 16777216.0) * 2.0 - 1.0) * amplitude;
        let bits = value.to_bits();
        f32::from_bits(bits.wrapping_add(0x7fff + ((bits >> 16) & 1)) & 0xffff0000)
    }).collect()
}

fn narlayer_time(name: &str, flops: f64, mut run: impl FnMut() -> Result<(), String>) -> Result<f64, String> {
    for _ in 0..3 { run()?; }
    gpu_synchronize()?;
    let start = Instant::now();
    for _ in 0..28 { run()?; }
    gpu_synchronize()?;
    let seconds = start.elapsed().as_secs_f64();
    println!("narlayer {name}: x28 {:.3} ms, layer {:.3} ms, {:.2} TFLOPS",
        seconds * 1e3, seconds * 1e3 / 28.0, 28.0 * flops / seconds / 1e12);
    Ok(seconds)
}

fn narlayer_compare(name: &str, result: &[f32], reference: &[f32]) {
    let nonfinite = result.iter().filter(|x| !x.is_finite()).count();
    if nonfinite != 0 {
        println!("narlayer {name}: rel_l2=inf, nonfinite={nonfinite}/{}", result.len());
    } else {
        compare(&format!("narlayer {name}"), result, reference);
    }
}

fn narlayer_bench(operation: &str) {
    use makepad_ai_common::backend::{
        gpu_alloc_bf16, gpu_alloc_f32, gpu_attention_gqa_flash_bf16_buf_into,
        gpu_copy_f32_to_bf16_into, gpu_linear_nt_cached_precision_into, gpu_linear_scratch,
        gpu_pool_cap_override, gpu_runtime_trim, gpu_weight_cache_ensure_bf16_precision_keyed,
        gpu_swiglu_value_gate_into, gpu_weight_cache_evict_prefix, GpuLinearPrecision,
    };
    const KEY: &str = "yue2-narlayer::weight";
    assert!(matches!(operation, "all" | "attention" | "linears"), "--narlayer-op all|attention|linears");
    let (m, kv) = (5323usize, 13472usize);
    // This is a standalone microbench beside a resident LLM. Free idle pool
    // buffers immediately; outputs/scratch are preallocated before warming.
    // Largest live device tensor/weight/scratch set is f16 up|gate+SwiGLU:
    // ~685 MB including pool rounding; CUDA context/library storage is extra.
    gpu_pool_cap_override(Some(0));
    println!("narlayer: q={m} kv={kv}, 16/8 heads x128, warm=3 timed=28; no model loaded");
    println!("linears include activation staging and output conversion; weights uploaded before warmup");
    println!("attention uses persistent bf16 Q/K/V; SwiGLU TFLOPS counts 5 scalar ops/output (including exp)");
    gpu_synchronize().unwrap();
    let free_before = gpu_perf_stats(false).mem_free_bytes;
    let mut peak_delta = 0;
    let mut linear_seconds = [0.0; 3];
    let mut attention_seconds = [0.0; 3];
    let mut total_flops = 0.0;
    for (name, k, n) in [("qkv", 2048, 4096), ("o", 2048, 2048),
        ("up_gate", 2048, 12288), ("down", 6144, 2048)] {
        if operation == "attention" { break; }
        let weights: Vec<u8> = narlayer_random(n * k, 11, (3.0 / k as f32).sqrt())
            .into_iter().flat_map(|v| ((v.to_bits() >> 16) as u16).to_le_bytes()).collect();
        let input = gpu_upload(&narlayer_random(m * k, 17, 3.0f32.sqrt()), m, k).unwrap();
        let output = gpu_alloc_f32(m, n).unwrap();
        let flops = 2.0 * m as f64 * k as f64 * n as f64;
        total_flops += flops;
        let mut reference = Vec::new();
        let mut activation_reference = Vec::new();
        let activation_flops = 5.0 * m as f64 * (n / 2) as f64;
        if name == "up_gate" { total_flops += activation_flops; }
        for (index, precision) in [GpuLinearPrecision::Bf16F32, GpuLinearPrecision::F16F16,
            GpuLinearPrecision::Fp8F32].into_iter().enumerate() {
            // Evict after each variant; no simultaneous bf16/f16/fp8 copies.
            let key = format!("{KEY}::{precision:?}");
            gpu_weight_cache_ensure_bf16_precision_keyed(&key, n, k, precision, || Ok(weights.clone())).unwrap();
            let scratch = gpu_linear_scratch(m, k, n, precision).unwrap();
            let timing = narlayer_time(&format!("{name} {precision:?}"), flops, || {
                gpu_linear_nt_cached_precision_into(&input, &scratch, &[(key.as_str(), n)], None, &output)
            });
            match timing {
                Ok(seconds) => linear_seconds[index] += seconds,
                Err(error) => {
                    assert!(index != 0, "bf16 baseline failed: {error}");
                    println!("narlayer {name} {precision:?}: UNAVAILABLE ({error}); no fallback or rel_l2");
                    linear_seconds[index] = f64::NAN;
                    gpu_synchronize().unwrap();
                    drop(scratch);
                    gpu_weight_cache_evict_prefix("yue2-narlayer::").unwrap();
                    continue;
                }
            }
            peak_delta = peak_delta.max(free_before.saturating_sub(gpu_perf_stats(false).mem_free_bytes));
            let result = gpu_download(&output).unwrap();
            if index == 0 {
                assert!(result.iter().all(|x| x.is_finite()), "nonfinite bf16 GEMM reference");
                reference = result;
                narlayer_compare(&format!("{name} {precision:?}"), &reference, &reference);
            } else {
                narlayer_compare(&format!("{name} {precision:?}"), &result, &reference);
            }
            if name == "up_gate" {
                let activation = gpu_alloc_f32(m, n / 2).unwrap();
                linear_seconds[index] += narlayer_time(&format!("swiglu after {precision:?}"), activation_flops, || {
                    gpu_swiglu_value_gate_into(&output, &activation)
                }).unwrap();
                peak_delta = peak_delta.max(free_before.saturating_sub(gpu_perf_stats(false).mem_free_bytes));
                let result = gpu_download(&activation).unwrap();
                if index == 0 {
                    assert!(result.iter().all(|x| x.is_finite()), "nonfinite bf16 SwiGLU reference");
                    activation_reference = result;
                    narlayer_compare("swiglu after Bf16F32", &activation_reference, &activation_reference);
                } else {
                    narlayer_compare(&format!("swiglu after {precision:?}"), &result, &activation_reference);
                }
            }
            drop(scratch);
            gpu_weight_cache_evict_prefix("yue2-narlayer::").unwrap();
        }
        drop((input, output));
        gpu_runtime_trim().unwrap();
    }
    if operation != "linears" {
        let upload = |rows, cols, seed| {
            let x = gpu_upload(&narlayer_random(rows * cols, seed, 3.0f32.sqrt()), rows, cols).unwrap();
            let bf16 = gpu_alloc_bf16(rows, cols).unwrap();
            gpu_copy_f32_to_bf16_into(&x, &bf16, 0).unwrap();
            gpu_synchronize().unwrap();
            bf16
        };
        let q = upload(m, 2048, 21);
        let k = upload(kv, 1024, 22);
        let v = upload(kv, 1024, 23);
        let output = gpu_alloc_f32(m, 2048).unwrap();
        let flops = 4.0 * 16.0 * m as f64 * kv as f64 * 128.0;
        let mut reference = Vec::new();
        total_flops += flops;
        for (index, precision) in [GpuAttentionPrecision::Bf16F32, GpuAttentionPrecision::F16Pv,
            GpuAttentionPrecision::F16QkPv].into_iter().enumerate() {
            let timing = narlayer_time(&format!("attention {precision:?}"), flops, || {
                gpu_attention_gqa_flash_bf16_buf_into(&q, &k, &v, &output, 16, 8,
                    1.0 / 128.0f32.sqrt(), false, precision)
            });
            match timing {
                Ok(seconds) => attention_seconds[index] = seconds,
                Err(error) => {
                    assert!(index != 0, "bf16 baseline failed: {error}");
                    println!("narlayer attention {precision:?}: UNAVAILABLE ({error}); no fallback or rel_l2");
                    attention_seconds[index] = f64::NAN;
                    gpu_synchronize().unwrap();
                    continue;
                }
            }
            peak_delta = peak_delta.max(free_before.saturating_sub(gpu_perf_stats(false).mem_free_bytes));
            let result = gpu_download(&output).unwrap();
            if precision == GpuAttentionPrecision::Bf16F32 {
                assert!(result.iter().all(|x| x.is_finite()), "nonfinite bf16 attention reference");
                reference = result;
                narlayer_compare("attention Bf16F32", &reference, &reference);
            } else {
                narlayer_compare(&format!("attention {precision:?}"), &result, &reference);
            }
        }
    }
    for (name, gemm, attention) in [("bf16", 0, 0), ("fast", 1, 1), ("fp8+PV16", 2, 1),
        ("f16+QKPV16 (measure only)", 1, 2)] {
        if operation != "all" { break; }
        let total = linear_seconds[gemm] + attention_seconds[attention];
        if !total.is_finite() {
            println!("narlayer {name} core sum: UNAVAILABLE");
            continue;
        }
        println!("narlayer {name} core sum x28: {:.3} ms, layer {:.3} ms, {:.2} TFLOPS (excludes norms/RoPE/residual)",
            total * 1e3, total * 1e3 / 28.0, 28.0 * total_flops / total / 1e12);
    }
    println!("narlayer sampled device allocation delta {:.1} MB (cudaMemGetInfo; other GPU processes can affect this)",
        peak_delta as f64 / 1e6);
    gpu_runtime_trim().unwrap();
    gpu_pool_cap_override(None);
}

// Both samplers consume the exact saved oracle logits and teacher history.
// This isolates RNG/filter parity from model numerical drift.
fn check_device_sampler(dir: &Path, seed: u64) {
    for (phase, file, tokens, preset) in [
        (Yue2Phase::Abc, "tf_abc_logits.f32", "abc.i32", Yue2Sampling::ABC),
        (Yue2Phase::Semantic, "tf_sem_logits.f32", "semantic.i32", Yue2Sampling::SEMANTIC),
    ] {
        let logits = read_f32(&dir.join(file));
        let mut ids = read_i32(&dir.join(tokens));
        if phase == Yue2Phase::Semantic { for id in &mut ids { *id += CODEC_OFFSET; } }
        let (lo, width) = phase.head_rows();
        assert_eq!(logits.len() % width as usize, 0);
        let rows = logits.len() / width as usize;
        assert!(rows > 0 && ids.len() >= rows);
        let mut unfiltered = preset;
        unfiltered.top_p = 1.0;
        for (name, sampling, legacy, draw_seed, guidance) in [
            ("preset", preset, false, seed, 1.0),
            ("seed64", preset, false, seed ^ 0xfedc_ba98_7654_3210, 1.0),
            ("greedy", Yue2Sampling::GREEDY, false, seed, 1.0),
            ("top-p=1", unfiltered, false, seed, 1.0),
            ("legacy", preset, true, seed, 1.0),
            ("legacy-cfg", preset, true, seed, 1.01),
        ] {
            let mut host = Yue2Sampler::new(draw_seed);
            let device = Yue2DeviceSampler::new(draw_seed, phase, &sampling, legacy, guidance).unwrap();
            for r in 0..rows {
                let scores = &logits[r * width as usize..(r + 1) * width as usize];
                let input = gpu_upload(scores, 1, width as usize).unwrap();
                // A second oracle row exercises the CFG arithmetic without
                // needing a separate negative-branch oracle dump.
                let uncond = (guidance != 1.0).then(|| {
                    let next = (r + 1) % rows;
                    &logits[next * width as usize..(next + 1) * width as usize]
                });
                let guided: Option<Vec<f32>> = uncond.map(|u| {
                    scores.iter().zip(u).map(|(&c, &u)| u + guidance * (c - u)).collect()
                });
                let uncond_device = uncond.map(|u| gpu_upload(u, 1, width as usize).unwrap());
                let expected = host.sample(guided.as_deref().unwrap_or(scores), lo, phase, &sampling, &ids[..r], r, legacy);
                device.sample(&input, uncond_device.as_ref()).unwrap();
                assert_eq!(device.read_token().unwrap(), expected, "{phase:?} {name} sampler row {r}");
                device.force_token(ids[r]).unwrap();
                device.accept_previous().unwrap();
            }
            println!("sample-device {phase:?} {name}: identical {rows}/{rows}");
        }
    }
}

// In graph mode this checks the actual replay output against the
// oracle, and checks its sampled id against the host on the SAME logits.
fn check_teacher_forced(
    pipe: &Yue2Pipeline, dir: &Path, phase: Yue2Phase, seed: u64,
) {
    let (prefix_file, token_file, logits_file, mut sampling) = match phase {
        Yue2Phase::Abc => ("prefix_abc.i32", "abc.i32", "tf_abc_logits.f32", Yue2Sampling::ABC),
        Yue2Phase::Semantic => ("prefix_sem.i32", "semantic.i32", "tf_sem_logits.f32", Yue2Sampling::SEMANTIC),
    };
    let prefix = read_i32(&dir.join(prefix_file));
    let mut ids = read_i32(&dir.join(token_file));
    if phase == Yue2Phase::Semantic { for id in &mut ids { *id += CODEC_OFFSET; } }
    let oracle = read_f32(&dir.join(logits_file));
    let (lo, width) = phase.head_rows();
    let width = width as usize;
    assert_eq!(oracle.len() % width, 0);
    let rows = oracle.len() / width;
    assert!(rows > 0 && ids.len() >= rows);
    sampling.max_tokens = rows + 1;
    let model = &pipe.model;
    let mut eager = None;
    let mut device = None;
    let mut sampled = 0;
    if model.decode_mode() == Yue2DecodeMode::Graph {
        let (session, token) = Yue2DecodeSession::new(model, &prefix, None, phase, &sampling, seed, 1.0, false).unwrap();
        device = Some(session);
        sampled = token;
    } else {
        let mut cache = Yue2KvCache::new(prefix.len() + rows + 1, model.ar_weights()).unwrap();
        let hidden = model.ar_forward(&mut cache, &prefix).unwrap();
        eager = Some((cache, hidden));
    }
    let mut host = Yue2Sampler::new(seed);
    let (mut worst, mut agree, mut overlap) = (0.0f64, 0usize, 0usize);
    for r in 0..rows {
        let logits = if let Some(session) = &device {
            gpu_download(session.logits().0).unwrap()
        } else {
            model.head_window(&eager.as_ref().unwrap().1, phase, r != 0).unwrap().1
        };
        if device.is_some() {
            let expected = host.sample(&logits, lo, phase, &sampling, &ids[..r], r, false);
            assert_eq!(sampled, expected, "{phase:?} native device sampler row {r}");
        }
        let refr = &oracle[r * width..(r + 1) * width];
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for (&a, &b) in logits.iter().zip(refr) {
            assert!(a.is_finite(), "{phase:?} nonfinite logit at row {r}");
            num += ((a - b) as f64).powi(2);
            den += (b as f64).powi(2);
        }
        worst = worst.max((num / den).sqrt());
        let range = match phase { Yue2Phase::Abc => 0..EOD as usize, Yue2Phase::Semantic => 1..width };
        agree += (argmax(&logits[range.clone()]) == argmax(&refr[range.clone()])) as usize;
        overlap += topk_overlap(&logits[range.clone()], &refr[range], 100);
        if r + 1 < rows {
            if let Some(session) = &mut device {
                session.force_token(ids[r]).unwrap();
                sampled = session.next_token().unwrap();
            } else {
                let (cache, hidden) = eager.as_mut().unwrap();
                *hidden = model.ar_forward(cache, &[ids[r]]).unwrap();
            }
        }
    }
    println!("tf {phase:?} {} weights={}: {rows} rows, worst rel_l2 {worst:.3e}, argmax {agree}/{rows}, mean top100 {:.1}",
        model.decode_mode().name(), model.ar_weights().name(), overlap as f64 / rows as f64);
    match phase {
        Yue2Phase::Abc => assert_eq!(agree, rows, "ABC teacher-forced argmax gate"),
        Yue2Phase::Semantic => {
            let (max_rel, min_agree) = (2.5e-2, 245);
            assert!(worst <= max_rel, "semantic teacher-forced rel_l2 gate: {worst}");
            assert!(agree * 256 >= min_agree * rows, "semantic teacher-forced argmax gate");
        }
    }
    if model.decode_mode() == Yue2DecodeMode::Graph && rows > 2 {
        assert!(device.as_ref().unwrap().is_captured(), "teacher-forced steps must exercise graph replay");
    }
}

fn bench_decode(pipe: &Yue2Pipeline, dir: &Path, phase: Yue2Phase, swiglu_variant: &str) {
    let model = &pipe.model;
    let (file, mut sampling) = match phase {
        Yue2Phase::Abc => ("prefix_abc.i32", Yue2Sampling::ABC),
        Yue2Phase::Semantic => ("prefix_sem.i32", Yue2Sampling::SEMANTIC),
    };
    let prefix = read_i32(&dir.join(file));
    let steps = 200usize;
    // Two untimed decode rows cover warmup and capture. Keep EOS masked
    // through the timed region so all modes measure the same row count.
    // Use the production cache capacity, including inactive graph chunks.
    sampling.max_tokens = sampling.max_tokens.max(steps + 3);
    sampling.min_tokens = sampling.min_tokens.max(steps + 3);
    let setup = Instant::now();
    if model.decode_mode() == Yue2DecodeMode::Graph {
        let (mut session, _) = Yue2DecodeSession::new(
            model, &prefix, None, phase, &sampling, 1, 1.0, false,
        ).unwrap();
        session.next_token().unwrap();
        session.next_token().unwrap();
        println!("bench setup {:.3}s, captured={}", setup.elapsed().as_secs_f64(), session.is_captured());
        let t = Instant::now();
        for _ in 0..steps { session.next_token().unwrap(); }
        let seconds = t.elapsed().as_secs_f64();
        println!("bench {phase:?} {} ctx {}..{}: forward+head+sample+4B readback {:.3} ms, {:.1} tok/s",
            model.decode_mode().name(), prefix.len() + 2, prefix.len() + steps + 2,
            seconds * 1e3 / steps as f64, steps as f64 / seconds);
        if model.ar_weights() == Yue2ArWeights::Q8 {
            let (_, profile) = session.profile_next_token().unwrap();
            report_decode_kernels(phase, prefix.len() + steps + 3, &profile);
            bench_swiglu(swiglu_variant);
        } else {
            println!("kernel breakdown requires Q8 graph decode; cuBLAS-internal launches are not instrumented");
        }
    } else {
        let mut cache = Yue2KvCache::new(prefix.len() + sampling.max_tokens, model.ar_weights()).unwrap();
        let mut hidden = model.ar_forward(&mut cache, &prefix).unwrap();
        let mut sampler = Yue2Sampler::new(1);
        let mut history = Vec::with_capacity(sampling.max_tokens);
        let (mut t_fwd, mut t_head, mut t_sample) = (0.0f64, 0.0f64, 0.0f64);
        for step in 0..steps + 2 {
            let t = Instant::now();
            let (lo, logits) = model.head_window(&hidden, phase, step != 0).unwrap();
            let head = t.elapsed().as_secs_f64();
            let t = Instant::now();
            let token = sampler.sample(&logits, lo, phase, &sampling, &history, step, false);
            let sample = t.elapsed().as_secs_f64();
            history.push(token);
            let t = Instant::now();
            hidden = model.ar_forward(&mut cache, &[token]).unwrap();
            gpu_download(&gpu_slice_cols(&hidden, 0, 1).unwrap()).unwrap();
            if step >= 2 {
                t_fwd += t.elapsed().as_secs_f64();
                t_head += head;
                t_sample += sample;
            }
        }
        let n = steps as f64;
        println!("bench {phase:?} {} ctx {}..{}: forward {:.3} ms, head+download {:.3} ms, host sample {:.3} ms => {:.1} tok/s",
            model.decode_mode().name(), prefix.len() + 2, prefix.len() + steps + 2,
            1e3 * t_fwd / n, 1e3 * t_head / n, 1e3 * t_sample / n, n / (t_fwd + t_head + t_sample));
        println!("use --ar-decode graph with MAKEPAD_YUE2_AR_WEIGHTS=q8 for the per-kernel decode breakdown");
    }
}

fn report_decode_kernels(phase: Yue2Phase, sequence: usize, profile: &GpuKernelProfile) {
    println!("bench {phase:?} kernel classes: one instrumented graph replay, live ctx {sequence}");
    report_kernel_classes(profile);
}

fn report_kernel_classes(profile: &GpuKernelProfile) {
    println!("{:32} {:>14} {:>7} {:>12} {:>12} {:>12} {:>10}",
        "class", "rows x cols", "count", "total us", "us/launch", "weight MB", "TB/s");
    for entry in &profile.classes {
        // A fused SwiGLU output row reads BOTH the up and gate weight rows.
        let matrices = match entry.class.as_str() {
            "q8.gemv" => 1,
            "q8.gemv_swiglu" => 2,
            _ => 0,
        };
        let bytes = matrices * u64::from(entry.rows) * u64::from(entry.cols) / 32 * 34;
        let us = entry.microseconds / entry.count as f64;
        let (mb, bandwidth) = if bytes != 0 && us > 0.0 {
            (format!("{:.3}", bytes as f64 / 1e6), format!("{:.3}", bytes as f64 / us / 1e6))
        } else { ("-".into(), "-".into()) };
        println!("{:32} {:>6} x {:<5} {:>7} {:>12.3} {:>12.3} {:>12} {:>10}",
            entry.class, entry.rows, entry.cols, entry.count, entry.microseconds,
            us, mb, bandwidth);
    }
    let count: usize = profile.classes.iter().map(|entry| entry.count).sum();
    println!("kernel-event total: {count} launches, {:.3} us; instrumented replay span: {:.3} us; event/dispatch gaps: {:.3} us",
        profile.kernel_microseconds, profile.span_microseconds,
        profile.span_microseconds - profile.kernel_microseconds);
    println!("Event nodes perturb short kernels; host submission stalls are excluded. Compare uninstrumented graph timings separately.");
    println!("Weight MB uses decimal bytes per launch; fused SwiGLU includes up + gate. sampler.history columns denote ring capacity, not vocabulary.");
}

// This choice belongs only to the bench. Production retains its fixed
// decode path; neither the model nor captured sessions gain a variant flag.
fn bench_swiglu_projection(input: &GpuTensor, key: &str, fused: bool) -> Result<GpuTensor, String> {
    if fused {
        gpu_linear_nt_cached_q8_0_swiglu_keyed(input, key, YUE2_FF)
    } else {
        let packed = gpu_linear_nt_cached_q8_0_f32acc_keyed(input, &[GpuLinearPart {
            bt_ggml_type: makepad_ai_common::quant::GGML_TYPE_Q8_0,
            n: 2 * YUE2_FF,
            cache_key: key,
            bytes: &[],
        }])?;
        gpu_swiglu_value_gate(&packed)
    }
}

fn bench_swiglu(variant: &str) {
    let variants: &[bool] = match variant {
        "both" => &[true, false],
        "fused" => &[true],
        "separate" => &[false],
        _ => panic!("--bench-swiglu both|fused|separate"),
    };
    // These are the real packed keys already warmed by the decode session.
    // Sweep all layers rather than repeatedly reading one matrix from L2.
    let keys: Vec<String> = (0..YUE2_LAYERS).map(|layer| {
        format!("{YUE2_NAMESPACE}::model.layers.{layer}.mlp.up_gate_proj::q8_0")
    }).collect();
    let values: Vec<f32> = (0..YUE2_HIDDEN)
        .map(|i| ((i * 17 % 257) as f32 - 128.0) / 64.0).collect();
    let input = gpu_upload(&values, 1, YUE2_HIDDEN).unwrap();
    for key in &keys {
        let mut reference: Option<Vec<f32>> = None;
        for &fused in variants {
            let output = bench_swiglu_projection(&input, key, fused).unwrap();
            let values = gpu_download(&output).unwrap();
            if let Some(reference) = &reference {
                let mismatches = reference.iter().zip(&values)
                    .filter(|(a, b)| a.to_bits() != b.to_bits()).count();
                assert_eq!(mismatches, 0, "SwiGLU variant bitwise mismatch: {key}");
            }
            reference = Some(values);
        }
    }
    if variants.len() == 2 {
        println!("SwiGLU variants: bitwise-identical outputs on all {YUE2_LAYERS} packed weights and the same nonzero input");
    }
    let run = |fused| -> Result<(), String> {
        for key in &keys {
            drop(bench_swiglu_projection(&input, key, fused)?);
        }
        Ok(())
    };
    let repetitions = 200;
    let bytes_per_layer = (2 * YUE2_FF * YUE2_HIDDEN / 32 * 34) as f64;
    let mut times = Vec::new();
    for &fused in variants {
        let label = if fused { "fused" } else { "separate" };
        let (graph, ()) = gpu_graph_capture(|| run(fused)).unwrap();
        gpu_graph_launch(&graph).unwrap();
        gpu_synchronize().unwrap();
        let start = Instant::now();
        for _ in 0..repetitions { gpu_graph_launch(&graph).unwrap(); }
        gpu_synchronize().unwrap();
        let us = start.elapsed().as_secs_f64() * 1e6 / (repetitions * YUE2_LAYERS) as f64;
        drop(graph);
        times.push((label, us));
        println!("SwiGLU {label}: uninstrumented graph sweep, {us:.3} us/layer, {:.3} weight MB/layer, {:.3} TB/s (GEMV + activation)",
            bytes_per_layer / 1e6, bytes_per_layer / us / 1e6);
        let (_, profile) = gpu_profile_kernels(2 * YUE2_LAYERS, || run(fused)).unwrap();
        println!("SwiGLU {label} kernel breakdown ({YUE2_LAYERS} layers):");
        report_kernel_classes(&profile);
    }
    if let [(a, ta), (b, tb)] = times.as_slice() {
        println!("SwiGLU comparison: {a} {ta:.3} us/layer; {b} {tb:.3} us/layer; measured faster: {}. Production uses fused; --bench-swiglu changes only this comparison.",
            if ta <= tb { a } else { b });
    }
}

fn report_device_work(label: &str, before: GpuPerfStats) {
    let after = gpu_perf_stats(false);
    println!(
        "{label}: fresh device allocations {}, weight uploads {}, evictions {}; device used at boundary {:.3} GiB (not peak)",
        after.pool_fresh_alloc_count - before.pool_fresh_alloc_count,
        after.weight_stream_count - before.weight_stream_count,
        after.weight_evict_events - before.weight_evict_events,
        after.mem_total_bytes.saturating_sub(after.mem_free_bytes) as f64 / (1u64 << 30) as f64,
    );
}

fn check_flow(pipe: &Yue2Pipeline, request: &Yue2Request, dir: &Path) {
    let prefix = read_i32(&dir.join("prefix_sem.i32"));
    let sem = read_i32(&dir.join("semantic.i32"));
    let refr = read_f32(&dir.join("latents.f32"));
    gpu_synchronize().unwrap();
    let t = Instant::now();
    let latents = pipe.synthesize(
        &prefix, &sem, request.seed, Yue2Pipeline::default_steps(), &mut |_| Ok(()),
    ).unwrap();
    println!("flow {} frames, {} midpoint steps (including prefill/setup) {:.3}s",
        sem.len(), Yue2Pipeline::default_steps(), t.elapsed().as_secs_f64());
    compare("flow latents", &latents, &refr);
}

/// Independent acoustic timings on oracle tokens/latents. Every interval is
/// stream-complete; the device loop has no per-eval upload or download.
fn acoustic_timing(pipe: &Yue2Pipeline, vae_dir: &Path, dir: &Path, evals: usize) {
    if pipe.graph_model.is_some() {
        return graph::acoustic(pipe, vae_dir, dir, evals);
    }
    assert!(evals > 0, "--velocity-evals must be positive");
    let request = load_request(&dir.join("fixture.json"));
    let mut ar = read_i32(&dir.join("prefix_sem.i32"));
    let sem = read_i32(&dir.join("semantic.i32"));
    let noise = read_f32(&dir.join("nar_noise.f32"));
    let frames = noise.len() / YUE2_LATENT_DIM;
    ar.extend(sem[..frames].iter().map(|&c| c + CODEC_OFFSET));
    ar.push(MUSIC_END);
    gpu_synchronize().unwrap();
    let t = Instant::now();
    let mut chunk = Yue2NarChunk::prefill(&pipe.model, &ar, frames, &[20.0]).unwrap();
    gpu_synchronize().unwrap();
    println!("acoustic prefill/setup: {} prefix + {} NAR tokens {:.3}s (includes cold NAR weights)",
        ar.len(), frames + 2, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let v0 = chunk.velocity(&noise, 0).unwrap();
    println!("velocity first eval {:.3} ms", t.elapsed().as_secs_f64() * 1e3);
    compare("nar v0", &v0, &read_f32(&dir.join("nar_v0.f32")));

    let before = gpu_perf_stats(false);
    let t = Instant::now();
    for _ in 0..evals {
        let _ = chunk.velocity(&noise, 0).unwrap();
    }
    let seconds = t.elapsed().as_secs_f64();
    println!("velocity warm with state upload/download: {evals} evals {seconds:.3}s, {:.3} ms/eval",
        1e3 * seconds / evals as f64);
    report_device_work("velocity host boundary", before);

    let state = gpu_bf16_round(&gpu_upload(&noise, frames, YUE2_LATENT_DIM).unwrap()).unwrap();
    gpu_synchronize().unwrap();
    let before = gpu_perf_stats(false);
    let t = Instant::now();
    for _ in 0..evals {
        let _ = chunk.velocity_device(&state, 0).unwrap();
    }
    gpu_synchronize().unwrap();
    let seconds = t.elapsed().as_secs_f64();
    println!("velocity warm device only: {evals} evals {seconds:.3}s, {:.3} ms/eval",
        1e3 * seconds / evals as f64);
    report_device_work("velocity device", before);
    drop((state, chunk));

    check_flow(pipe, &request, dir);
    let t = Instant::now();
    let vae = Yue2Vae::load(vae_dir).unwrap();
    println!("vae load/weight packing {:.3}s", t.elapsed().as_secs_f64());
    let latents = read_f32(&dir.join("latents.f32"));
    let refr = read_f32(&dir.join("audio.f32"));
    for label in ["cold weights", "warm weights"] {
        gpu_synchronize().unwrap();
        let t = Instant::now();
        let audio = vae.decode(&latents, &mut |_| Ok(())).unwrap();
        println!("vae {label}: {} frames {:.3}s", latents.len() / YUE2_LATENT_DIM, t.elapsed().as_secs_f64());
        let rel = compare("vae audio", &audio, &refr);
        println!("vae SNR {:.2} dB", -20.0 * rel.log10());
    }
}

fn check(pipe: &Yue2Pipeline, vae_dir: Option<&Path>, dir: &Path, stage: &str, bench_phase: Yue2Phase) {
    if pipe.graph_model.is_some() {
        return graph::check(pipe, vae_dir, dir, stage, bench_phase);
    }
    let fixture = dir.join("fixture.json");
    let request = load_request(&fixture);
    let all = stage == "all";
    let model = &pipe.model;

    if stage == "flow" {
        check_flow(pipe, &request, dir);
        return;
    }

    let prefix_ref = read_i32(&dir.join("prefix_abc.i32"));
    if all || stage == "tok" {
        let ours = pipe.tokenizer.encode(&request.style);
        let refr = read_i32(&dir.join("tok_style.i32"));
        println!("tok style: equal={} ({} ids)", ours == refr, refr.len());
        let ours = pipe.tokenizer.encode(&request.lyrics);
        let refr = read_i32(&dir.join("tok_lyrics.i32"));
        println!("tok lyrics: equal={} ({} ids)", ours == refr, refr.len());
        let abc = std::fs::read_to_string(dir.join("abc.txt")).unwrap();
        let ours = pipe.tokenizer.encode(&abc);
        let refr = read_i32(&dir.join("tok_abc.i32"));
        println!("tok abc: equal={} ({} ids)", ours == refr, refr.len());
        let ours = makepad_ai_music::yue2::yue2_prefix(&pipe.text_ids(&request), request.cot, None);
        println!("prefix: equal={} ({} ids)", ours == prefix_ref, prefix_ref.len());
        let abc_ids = read_i32(&dir.join("abc.i32"));
        let decoded = pipe.tokenizer.decode(&abc_ids);
        println!("decode abc: equal={}", decoded == abc);
        let sem = read_i32(&dir.join("semantic.i32"));
        println!(
            "abc duration estimate {:?} s vs oracle song {:.1} s",
            makepad_ai_music::yue2::yue2_abc_seconds(&abc),
            sem.len() as f64 / 25.0
        );
    }
    if all || stage == "prefill" {
        let refr = read_f32(&dir.join("abc_prefill_logits.f32"));
        let mut cache = Yue2KvCache::new(prefix_ref.len() + 1, model.ar_weights()).unwrap();
        let t = Instant::now();
        let hidden = model.ar_forward(&mut cache, &prefix_ref).unwrap();
        let (lo, logits) = model.head_window(&hidden, Yue2Phase::Abc, false).unwrap();
        println!("prefill {} tokens in {:.3}s", prefix_ref.len(), t.elapsed().as_secs_f64());
        let r = &refr[lo as usize..lo as usize + logits.len()];
        let rel = compare("prefill logits", &logits, r);
        assert!(rel <= 4e-3, "prefill rel_l2 gate: {rel}");
        assert_eq!(topk_overlap(&logits[..EOD as usize], &r[..EOD as usize], 50), 50, "prefill top50 gate");
        println!(
            "prefill argmax ours {} ref {}  top50 overlap {}/50",
            argmax(&logits[..EOD as usize]),
            argmax(&r[..EOD as usize]),
            topk_overlap(&logits[..EOD as usize], &r[..EOD as usize], 50)
        );
    }
    if all || stage == "greedy" {
        let refr = read_i32(&dir.join("greedy_abc.i32"));
        let (ids, _, timing) = pipe
            .generate(&prefix_ref, &Yue2Sampling::GREEDY, request.seed, Yue2Phase::Abc, 1.0, None, false, &mut |_| Ok(()))
            .unwrap();
        println!(
            "greedy abc: match {}/{} (ours {} tokens, {:.1} tok/s)",
            prefix_match(&ids, &refr),
            refr.len(),
            ids.len(),
            timing.tokens_per_second()
        );
        let matched = prefix_match(&ids, &refr);
        if matched < refr.len() {
            println!("greedy first divergence at token index {matched}: ours {:?}, reference {}",
                ids.get(matched), refr[matched]);
        }
        assert_eq!(matched, refr.len(), "greedy gate");
    }
    if all || stage == "tf" {
        check_teacher_forced(pipe, dir, Yue2Phase::Abc, request.seed);
        check_teacher_forced(pipe, dir, Yue2Phase::Semantic, request.seed);
    }
    if all || stage == "sample-device" {
        check_device_sampler(dir, request.seed);
    }
    if all || stage == "sample" {
        let abc_ref = read_i32(&dir.join("abc.i32"));
        let plan = pipe.plan(&request, &Yue2Sampling::ABC, &mut |_| Ok(())).unwrap();
        println!(
            "sampled abc: identical prefix {}/{} (ours {} tokens, {:.1} tok/s, prefill {:.3}s)",
            prefix_match(&plan.abc_ids, &abc_ref),
            abc_ref.len(),
            plan.abc_ids.len(),
            plan.timing.tokens_per_second(),
            plan.timing.prefill_seconds
        );
        // Semantic sampling from the ORACLE plan, so the two stages are
        // checked independently.
        let mut oracle_plan = plan.clone();
        oracle_plan.abc_ids = abc_ref.clone();
        oracle_plan.prefix = read_i32(&dir.join("prefix_sem.i32"));
        let sem_ref = read_i32(&dir.join("semantic.i32"));
        let mut sampling = Yue2Sampling::SEMANTIC;
        sampling.max_tokens = sampling.max_tokens.min(sem_ref.len() + 50);
        let sem = pipe.semantic(&request, &oracle_plan, &sampling, &mut |_| Ok(())).unwrap();
        println!(
            "sampled semantic: identical prefix {}/{} (ours {} tokens, {:.1} tok/s)",
            prefix_match(&sem.tokens, &sem_ref),
            sem_ref.len(),
            sem.tokens.len(),
            sem.timing.tokens_per_second()
        );
    }
    if stage == "attnbench" {
        // Kernel-level split of one NAR velocity eval: 28 x GQA flash over
        // (5323 queries, 13472 keys) vs the layer GEMMs at 5323 rows.
        use makepad_ai_common::backend::{gpu_attention_gqa_flash_bf16, gpu_download, gpu_slice_cols, gpu_upload};
        let (nq, nk) = (5323usize, 13472usize);
        let rnd = |n: usize, seed: u64| -> Vec<f32> {
            let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
            (0..n).map(|_| { x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((x >> 33) as f32 / 2147483648.0) - 1.0 }).collect()
        };
        let q = gpu_upload(&rnd(nq * 2048, 1), nq, 2048).unwrap();
        let k = gpu_upload(&rnd(nk * 1024, 2), nk, 1024).unwrap();
        let v = gpu_upload(&rnd(nk * 1024, 3), nk, 1024).unwrap();
        let sync = |t: &makepad_ai_common::backend::GpuTensor| { let _ = gpu_download(&gpu_slice_cols(t, 0, 1).unwrap()).unwrap(); };
        let o = gpu_attention_gqa_flash_bf16(&q, &k, &v, 16, 8, 0.088, false, GpuAttentionPrecision::Bf16F32).unwrap();
        sync(&o);
        let t = Instant::now();
        let mut last = o;
        for _ in 0..28 { last = gpu_attention_gqa_flash_bf16(&q, &k, &v, 16, 8, 0.088, false, GpuAttentionPrecision::Bf16F32).unwrap(); }
        sync(&last);
        let dt = t.elapsed().as_secs_f64();
        let flops = 28.0 * 16.0 * nq as f64 * nk as f64 * 128.0 * 4.0;
        println!("attention x28: {:.1} ms ({:.0} TFLOPS)", dt * 1e3, flops / dt / 1e12);
        return;
    }
    if stage == "synth" {
        // End-to-end acoustic exactness: the oracle's own prefix + codec
        // tokens + seed through our flow matching and VAE.
        let prefix_sem = read_i32(&dir.join("prefix_sem.i32"));
        let sem = read_i32(&dir.join("semantic.i32"));
        let t = Instant::now();
        let latents = pipe
            .synthesize(&prefix_sem, &sem, request.seed, Yue2Pipeline::default_steps(), &mut |_| Ok(()))
            .unwrap();
        println!("synth flow {:.2}s", t.elapsed().as_secs_f64());
        let rel = compare("synth latents", &latents, &read_f32(&dir.join("latents.f32")));
        println!("synth latents SNR {:.1} dB", -20.0 * rel.log10());
        if model.nar_precision() == Yue2NarPrecision::Fast {
            assert!(rel <= 5.5e-2, "fast NAR synth latents gate: {rel}");
        }
        let vae = Yue2Vae::load(vae_dir.expect("--vae DIR")).unwrap();
        let audio = vae.decode(&latents, &mut |_| Ok(())).unwrap();
        let rel = compare("synth audio", &audio, &read_f32(&dir.join("audio.f32")));
        println!("synth audio SNR {:.1} dB", -20.0 * rel.log10());
        return;
    }
    if all || stage == "vae" {
        check_vae(vae_dir.expect("--vae DIR"), dir, None);
    }
    if all || stage == "nar" {
        let prefix_sem = read_i32(&dir.join("prefix_sem.i32"));
        let sem = read_i32(&dir.join("semantic.i32"));
        let noise_ref = read_f32(&dir.join("nar_noise.f32"));
        let ours_noise = torch_cpu_randn(request.seed, sem.len() * YUE2_LATENT_DIM);
        let n = noise_ref.len();
        let noise_rel = compare("nar noise (chunk 0)", &ours_noise[..n], &noise_ref);
        // torch draws with a vectorised log/cos/sin: last-ulp differences only.
        assert!(noise_rel <= 1e-6, "NAR noise gate: {noise_rel}");
        let frames = n / YUE2_LATENT_DIM;
        let mut ar = prefix_sem.clone();
        ar.extend(sem[..frames].iter().map(|&c| c + CODEC_OFFSET));
        ar.push(MUSIC_END);
        let t = Instant::now();
        let mut chunk = Yue2NarChunk::prefill(model, &ar, frames, &[20.0]).unwrap();
        gpu_synchronize().unwrap();
        println!("nar prefill/setup {} tokens {:.3}s (includes NAR weight upload)", ar.len(), t.elapsed().as_secs_f64());
        let t = Instant::now();
        let v0 = chunk.velocity(&noise_ref, 0).unwrap();
        println!("nar velocity eval (first) {:.3}s", t.elapsed().as_secs_f64());
        let t = Instant::now();
        let _ = chunk.velocity(&noise_ref, 0).unwrap();
        println!("nar velocity eval (warm) {:.3}s", t.elapsed().as_secs_f64());
        let v0_rel = compare("nar v0", &v0, &read_f32(&dir.join("nar_v0.f32")));
        let limit = if model.nar_precision() == Yue2NarPrecision::Fast { 1.5e-2 } else { 1.3e-2 };
        assert!(v0_rel <= limit, "NAR {} v0 gate: {v0_rel} > {limit}", model.nar_precision().name());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if arg(&args, "--stage").as_deref() == Some("narlayer") {
        narlayer_bench(arg(&args, "--narlayer-op").as_deref().unwrap_or("all"));
        return;
    }
    let vae_frames = if args.iter().any(|a| a == "--vae-frames") {
        let n: usize = arg(&args, "--vae-frames").expect("--vae-frames N")
            .parse().expect("--vae-frames N");
        assert!(n > 0, "--vae-frames must be positive");
        Some(n)
    } else { None };
    if let Some(dir) = arg(&args, "--check") {
        if arg(&args, "--stage").as_deref() == Some("vae") {
            // The VAE gate needs no transformer weights or AR/NAR arenas.
            let vae_dir = PathBuf::from(arg(&args, "--vae").expect("--vae DIR"));
            check_vae(&vae_dir, Path::new(&dir), vae_frames);
            return;
        }
    }
    assert!(vae_frames.is_none(), "--vae-frames requires --check DIR --stage vae");
    let model_dir = PathBuf::from(arg(&args, "--model").expect("--model DIR"));
    let t = Instant::now();
    let pipe = Yue2Pipeline::load(&model_dir).expect("load");
    let mode = arg(&args, "--ar-decode").unwrap_or_else(|| "graph".into());
    let mode = Yue2DecodeMode::parse(&mode).expect("--ar-decode graph|host");
    pipe.model.set_decode_mode(mode);
    eprintln!("loaded in {:.2}s", t.elapsed().as_secs_f64());
    eprintln!("NAR precision={}", pipe.model.nar_precision().name());
    eprintln!("AR weights={}, KV={}, additional Q8 weights when resident: {} bytes ({:.3} GiB)",
        pipe.model.ar_weights().name(),
        if pipe.model.ar_weights() == Yue2ArWeights::Q8 { "bf16" } else { "f32" },
        pipe.model.ar_q8_extra_bytes(), pipe.model.ar_q8_extra_bytes() as f64 / 1073741824.0);
    if let Some(dir) = arg(&args, "--check") {
        let stage = arg(&args, "--stage").unwrap_or_else(|| "all".into());
        let vae_dir = arg(&args, "--vae").map(PathBuf::from);
        let bench_phase = match arg(&args, "--bench-phase").as_deref().unwrap_or("semantic") {
            "abc" => Yue2Phase::Abc,
            "semantic" => Yue2Phase::Semantic,
            _ => panic!("--bench-phase abc|semantic"),
        };
        if pipe.graph_model.is_some() {
            // The graph backend (Metal, or forced) has its own check stages.
            check(&pipe, vae_dir.as_deref(), Path::new(&dir), &stage, bench_phase);
            return;
        }
        if stage == "acoustic" {
            let evals = arg(&args, "--velocity-evals").map(|n| n.parse().expect("--velocity-evals N"))
                .unwrap_or(64);
            acoustic_timing(&pipe, vae_dir.as_deref().expect("--vae DIR"), Path::new(&dir), evals);
            return;
        }
        if stage == "bench" {
            let phase = match arg(&args, "--bench-phase").as_deref().unwrap_or("semantic") {
                "abc" => Yue2Phase::Abc,
                "semantic" => Yue2Phase::Semantic,
                _ => panic!("--bench-phase abc|semantic"),
            };
            let variant = arg(&args, "--bench-swiglu").unwrap_or_else(|| "both".into());
            assert!(matches!(variant.as_str(), "both" | "fused" | "separate"),
                "--bench-swiglu both|fused|separate");
            bench_decode(&pipe, Path::new(&dir), phase, &variant);
            return;
        }
        check(&pipe, vae_dir.as_deref(), Path::new(&dir), &stage, bench_phase);
        return;
    }
    let fixture = PathBuf::from(arg(&args, "--fixture").expect("--fixture json"));
    let out = PathBuf::from(arg(&args, "--out").expect("--out DIR"));
    std::fs::create_dir_all(&out).unwrap();
    let request = load_request(&fixture);
    let mut semantic_sampling = Yue2Sampling::SEMANTIC;
    if let Some(n) = arg(&args, "--max-semantic") {
        semantic_sampling.max_tokens = n.parse().unwrap();
        semantic_sampling.min_tokens = semantic_sampling.min_tokens.min(semantic_sampling.max_tokens);
    }
    let mut last = Instant::now();
    let mut progress = |p: Yue2Progress| {
        if last.elapsed().as_secs_f64() > 2.0 {
            eprintln!("{p:?}");
            last = Instant::now();
        }
        Ok(())
    };
    let total = Instant::now();
    let plan = pipe.plan(&request, &Yue2Sampling::ABC, &mut progress).expect("plan");
    eprintln!(
        "score: {} tokens {:.2}s ({:.1} tok/s)",
        plan.abc_ids.len(),
        plan.timing.seconds,
        plan.timing.tokens_per_second()
    );
    std::fs::write(out.join("score.abc"), plan.abc.clone().unwrap_or_default()).unwrap();
    write_i32(&out.join("abc.i32"), &plan.abc_ids);
    let sem = pipe.semantic(&request, &plan, &semantic_sampling, &mut progress).expect("semantic");
    eprintln!(
        "codec: {} tokens {:.2}s ({:.1} tok/s) = {:.1}s audio",
        sem.tokens.len(),
        sem.timing.seconds,
        sem.timing.tokens_per_second(),
        sem.tokens.len() as f64 / 25.0
    );
    write_i32(&out.join("semantic.i32"), &sem.tokens);
    let t = Instant::now();
    let latents = pipe
        .synthesize(&plan.prefix, &sem.tokens, request.seed, Yue2Pipeline::default_steps(), &mut progress)
        .expect("synthesize");
    eprintln!("flow: {:.2}s", t.elapsed().as_secs_f64());
    write_f32(&out.join("latents.f32"), &latents);
    let vae = Yue2Vae::load(arg(&args, "--vae").expect("--vae DIR")).expect("vae");
    let t = Instant::now();
    let audio = vae.decode(&latents, &mut progress).expect("decode");
    eprintln!("vae: {:.2}s", t.elapsed().as_secs_f64());
    write_wav(&out.join("song.wav"), &audio, 48_000);
    eprintln!(
        "total {:.2}s for {:.1}s of audio",
        total.elapsed().as_secs_f64(),
        audio.len() as f64 / 2.0 / 48_000.0
    );
}
