//! Open the official audio.cpp Music3 Q4 pack and check it against the
//! Python ModularPipeline contract (names, shapes, prompt assembly).
//! Does not run the CUDA safetensors path.
//!
//!   music3-gguf-inspect --weights /Users/dev/metal-probe/music3

use makepad_diffusion::h3_tokenizer::H3Tokenizer;
use makepad_diffusion::music3::{
    assemble_prompt, load_tokenizer, tokenize_cfg_pair, MUSIC3_AUDIO_CFG_TOKEN_ID,
    MUSIC3_AUDIO_START_ID, MUSIC3_IM_END_ID, MUSIC3_IM_START_ID, MUSIC3_LM_HIDDEN,
    MUSIC3_LM_RMS_EPS, MUSIC3_PINE_LYRICS, MUSIC3_PINE_PROMPT, MUSIC3_RVQ_HIDDEN,
};
use makepad_diffusion::music3_gguf::{topk_ids, FiniteStats, Music3GgufLm, Music3GgufRvq};
use makepad_diffusion::music3_quant::{Music3GgufPack, Music3GgufRole};
use makepad_ai_common::quant::dequantize_q4_0;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let weights = args
        .windows(2)
        .find(|w| w[0] == "--weights")
        .map(|w| PathBuf::from(&w[1]))
        .unwrap_or_else(|| PathBuf::from("local/models/music3"));
    let skip_prefill = args.iter().any(|a| a == "--skip-prefill");
    let lm_layers = args
        .windows(2)
        .find(|w| w[0] == "--lm-layers")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(1usize);

    println!("pack {}", weights.display());
    let t0 = Instant::now();
    let pack = Music3GgufPack::open(&weights).unwrap_or_else(|err| {
        eprintln!("open: {err}");
        std::process::exit(1);
    });
    println!("open {:.2}s", t0.elapsed().as_secs_f32());

    for role in [
        Music3GgufRole::Condition,
        Music3GgufRole::Vocoder,
        Music3GgufRole::Transformer,
        Music3GgufRole::Rvq,
        Music3GgufRole::LanguageModel,
    ] {
        let file = pack.file(role);
        println!(
            "  {} tensors={} arch={} family={} names={} dtypes={:?}",
            role.as_str(),
            file.tensor_count(),
            file.architecture,
            file.family,
            file.name_format,
            file.dtype_counts()
        );
    }

    match pack.validate_python_shapes() {
        Ok(rows) => {
            println!("shapes {} canaries ok", rows.len());
            for row in &rows {
                println!("  {row}");
            }
        }
        Err(err) => {
            eprintln!("shapes: {err}");
            std::process::exit(1);
        }
    }

    let tok_dir = pack.paths.tokenizer.clone();
    println!("tokenizer {}", tok_dir.display());
    let tokenizer = load_tokenizer(&tok_dir).unwrap_or_else(|err| {
        eprintln!("tokenizer: {err}");
        std::process::exit(1);
    });
    let assembled = assemble_prompt(MUSIC3_PINE_PROMPT, MUSIC3_PINE_LYRICS);
    println!("assembled {} chars", assembled.len());
    let mut token_ids: Option<Vec<u32>> = None;
    match tokenize_cfg_pair(&tokenizer, MUSIC3_PINE_PROMPT, MUSIC3_PINE_LYRICS) {
        Ok(pairs) => {
            println!(
                "tokens {} first={} last=[{}, {}] cfg={}",
                pairs.len(),
                pairs[0][0],
                pairs[pairs.len() - 2][0],
                pairs[pairs.len() - 1][0],
                pairs.get(1).map(|p| p[1]).unwrap_or(0)
            );
            if pairs[0][0] != MUSIC3_IM_START_ID
                || pairs[pairs.len() - 2][0] != MUSIC3_IM_END_ID
                || pairs[pairs.len() - 1][0] != MUSIC3_AUDIO_START_ID
                || pairs[1][1] != MUSIC3_AUDIO_CFG_TOKEN_ID
            {
                eprintln!("specials mismatch vs official python ids");
                std::process::exit(1);
            }
            print_specials(&tokenizer);
            let ids: Vec<u32> = pairs.iter().map(|p| p[0]).collect();
            token_ids = Some(ids.clone());
            match pack
                .language_model
                .gather_rows("model.embed_tokens.weight", &ids)
            {
                Ok(emb) => {
                    let (finite, peak, mean) = f32_stats(&emb);
                    println!(
                        "embed pine {}x{MUSIC3_LM_HIDDEN} finite={finite} peak={peak:.4} mean={mean:.4}",
                        ids.len()
                    );
                    match pack.language_model.read_f32_any("model.layers.0.input_layernorm.weight")
                    {
                        Ok(gamma) => {
                            let mut normed = emb;
                            rms_norm_mul(&mut normed, &gamma, MUSIC3_LM_HIDDEN, MUSIC3_LM_RMS_EPS);
                            let (finite, peak, mean) = f32_stats(&normed);
                            println!(
                                "rms L0 pine finite={finite} peak={peak:.4} mean={mean:.4}"
                            );
                        }
                        Err(err) => eprintln!("rms gamma: {err}"),
                    }
                }
                Err(err) => eprintln!("embed pine: {err}"),
            }
        }
        Err(err) => {
            eprintln!("tokenize: {err}");
            std::process::exit(1);
        }
    }

    match pack.load_condition_encoder() {
        Ok(enc) => {
            let frames = 8;
            let dummy = vec![0f32; frames * 8 * 4096];
            match enc.forward(&dummy, frames) {
                Ok(out) => println!(
                    "cond-encoder logits={} scale={:.4} proj={} dummy_out={}",
                    enc.layer_weight_logits.len(),
                    enc.layer_scale,
                    enc.proj_weight.len(),
                    out.len()
                ),
                Err(err) => {
                    eprintln!("cond forward: {err}");
                    std::process::exit(1);
                }
            }
        }
        Err(err) => {
            eprintln!("cond load: {err}");
            std::process::exit(1);
        }
    }

    match pack
        .language_model
        .read_bytes("model.layers.0.self_attn.q_proj.weight")
    {
        Ok(bytes) => {
            let d0 = u16::from_le_bytes([bytes[0], bytes[1]]);
            let nz = bytes.iter().filter(|b| **b != 0).count();
            println!(
                "q_proj bytes={} first_scale_bits={d0:#06x} nonzero={nz}/{}",
                bytes.len(),
                bytes.len()
            );
            let mut row0 = vec![0f32; 4096];
            for i in 0..128 {
                dequantize_q4_0(&bytes[i * 18..i * 18 + 18], &mut row0[i * 32..i * 32 + 32]);
            }
            let cpu_sum: f64 = row0.iter().map(|v| *v as f64).sum();
            let (finite, peak, mean) = f32_stats(&row0);
            println!(
                "cpu q_proj row0 dequant finite={finite} peak={peak:.6e} mean={mean:.6e} sum={cpu_sum:.6e}"
            );
        }
        Err(err) => eprintln!("q_proj bytes: {err}"),
    }

    // Official Metal Q4_0 GEMM: m=1 hits mul_mv, m=16 hits mul_mm.
    let ones = vec![1f32; 4096];
    let ones16 = vec![1f32; 4096 * 16];
    let t1 = Instant::now();
    match pack
        .language_model
        .linear_nt("model.layers.0.self_attn.q_proj.weight", &ones, 1)
    {
        Ok(out) => {
            let (finite, peak, mean) = f32_stats(&out);
            println!(
                "metal q_proj Q4_0 m=1 {:.3}s finite={finite}/{} peak={peak:.6e} mean={mean:.6e} head={:?}",
                t1.elapsed().as_secs_f32(),
                out.len(),
                &out[..8.min(out.len())]
            );
        }
        Err(err) => {
            eprintln!("metal q_proj m=1: {err}");
            std::process::exit(1);
        }
    }
    let t1b = Instant::now();
    match pack
        .language_model
        .linear_nt("model.layers.0.self_attn.q_proj.weight", &ones16, 16)
    {
        Ok(out) => {
            let (finite, peak, mean) = f32_stats(&out);
            println!(
                "metal q_proj Q4_0 m=16 {:.3}s finite={finite}/{} peak={peak:.6e} mean={mean:.6e} head={:?}",
                t1b.elapsed().as_secs_f32(),
                out.len(),
                &out[..8.min(out.len())]
            );
        }
        Err(err) => {
            eprintln!("metal q_proj m=16: {err}");
            std::process::exit(1);
        }
    }
    let t2 = Instant::now();
    let dit_in = vec![1f32; 2048];
    match pack
        .transformer
        .linear_nt("transformer_blocks.0.attn.to_q.weight", &dit_in, 1)
    {
        Ok(out) => {
            let (finite, peak, mean) = f32_stats(&out);
            println!(
                "metal dit.to_q Q4_0 1x2048x2048 {:.3}s finite={finite}/{} peak={peak:.4} mean={mean:.4}",
                t2.elapsed().as_secs_f32(),
                out.len()
            );
        }
        Err(err) => {
            eprintln!("metal dit.to_q: {err}");
            std::process::exit(1);
        }
    }

    match pack
        .language_model
        .gather_f16_rows("model.embed_tokens.weight", &[MUSIC3_IM_START_ID])
    {
        Ok(row) => {
            let (finite, peak, mean) = f32_stats(&row);
            println!(
                "embed im_start F16 hidden={} finite={finite} peak={peak:.4} mean={mean:.4}",
                row.len()
            );
        }
        Err(err) => {
            eprintln!("embed: {err}");
            std::process::exit(1);
        }
    }

    match pack.load_vocoder() {
        Ok(voc) => {
            let frames = 4;
            let latents = vec![0.01f32; 128 * frames];
            let t3 = Instant::now();
            match voc.decode(&latents, frames) {
                Ok(audio) => {
                    let (finite, peak, mean) = f32_stats(&audio);
                    println!(
                        "vocoder 4-latent {:.3}s samples={} finite={finite} peak={peak:.4} mean={mean:.4}",
                        t3.elapsed().as_secs_f32(),
                        audio.len()
                    );
                }
                Err(err) => {
                    eprintln!("vocoder decode: {err}");
                    std::process::exit(1);
                }
            }
        }
        Err(err) => {
            eprintln!("vocoder load: {err}");
            std::process::exit(1);
        }
    }

    match pack.rvq.read_f32_any("layers.0.input_layernorm.weight") {
        Ok(gamma) => {
            let s = FiniteStats::of(&gamma);
            println!("rvq L0 rms BF16 {s}");
        }
        Err(err) => {
            eprintln!("rvq rms: {err}");
            std::process::exit(1);
        }
    }
    let rvq_in = vec![0.01f32; 2 * MUSIC3_RVQ_HIDDEN];
    let t_rvq_gemm = Instant::now();
    match pack.rvq.linear_nt("layers.0.attn.to_q.weight", &rvq_in, 2) {
        Ok(out) => {
            let s = FiniteStats::of(&out);
            println!(
                "metal rvq.to_q BF16 2x4096x4096 {:.3}s {s}",
                t_rvq_gemm.elapsed().as_secs_f32()
            );
        }
        Err(err) => {
            eprintln!("metal rvq.to_q: {err}");
            std::process::exit(1);
        }
    }
    match Music3GgufRvq::load(&pack.rvq) {
        Ok(rvq) => {
            let t4 = Instant::now();
            match rvq.forward(&pack.rvq, &rvq_in, 2) {
                Ok(out) => {
                    let s = FiniteStats::of(&out);
                    println!(
                        "rvq 4-layer seq=2 {:.3}s {s}",
                        t4.elapsed().as_secs_f32()
                    );
                }
                Err(err) => {
                    eprintln!("rvq forward: {err}");
                    std::process::exit(1);
                }
            }
        }
        Err(err) => {
            eprintln!("rvq load: {err}");
            std::process::exit(1);
        }
    }

    if !skip_prefill {
        if let Some(ids) = token_ids {
            match Music3GgufLm::prepare(&pack.language_model) {
                Ok(lm) => {
                    let t5 = Instant::now();
                    match lm.prefill(&pack.language_model, &ids, lm_layers) {
                        Ok(hidden) => {
                            let last = &hidden[(ids.len() - 1) * MUSIC3_LM_HIDDEN..];
                            let s = FiniteStats::of(last);
                            println!(
                                "lm prefill layers={lm_layers} tokens={} {:.3}s last {s}",
                                ids.len(),
                                t5.elapsed().as_secs_f32()
                            );
                            let t6 = Instant::now();
                            match lm.head_last(&pack.language_model, &hidden, ids.len()) {
                                Ok(logits) => {
                                    let s = FiniteStats::of(&logits);
                                    let top = topk_ids(&logits, 8);
                                    println!(
                                        "lm_head last {:.3}s {s} top8={top:?}",
                                        t6.elapsed().as_secs_f32()
                                    );
                                }
                                Err(err) => {
                                    eprintln!("lm_head: {err}");
                                    std::process::exit(1);
                                }
                            }
                        }
                        Err(err) => {
                            eprintln!("lm prefill: {err}");
                            std::process::exit(1);
                        }
                    }
                }
                Err(err) => {
                    eprintln!("lm prepare: {err}");
                    std::process::exit(1);
                }
            }
        }
    }

    let _ = assembled;
    println!("ok (python-contract + mac gguf runtime; no CUDA generate)");
}

fn f32_stats(x: &[f32]) -> (usize, f32, f32) {
    let mut finite = 0usize;
    let mut peak = 0f32;
    let mut sum = 0f64;
    for &v in x {
        if v.is_finite() {
            finite += 1;
            peak = peak.max(v.abs());
            sum += v as f64;
        }
    }
    (finite, peak, (sum / x.len().max(1) as f64) as f32)
}

fn rms_norm_mul(x: &mut [f32], gamma: &[f32], width: usize, eps: f32) {
    for row in x.chunks_mut(width) {
        let mut ss = 0f32;
        for v in row.iter() {
            ss += *v * *v;
        }
        let inv = (ss / width as f32 + eps).sqrt().recip();
        for (v, g) in row.iter_mut().zip(gamma.iter()) {
            *v *= inv * *g;
        }
    }
}

fn print_specials(tokenizer: &H3Tokenizer) {
    for name in [
        "<|im_start|>",
        "<|im_end|>",
        "<|caption_start|>",
        "<|caption_end|>",
        "<|lyrics_start|>",
        "<|lyrics_end|>",
        "<|audio_start|>",
    ] {
        let ids = tokenizer.encode(name);
        println!("  special {name} -> {ids:?}");
    }
}
