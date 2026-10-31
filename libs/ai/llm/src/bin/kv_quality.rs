//! KV-cache precision quality probe: perplexity of a fixed sample at depth.
//!
//! ```text
//! kv-quality <model.gguf> --kv f16|q8 --text <file> [--prefix 8192] [--score 1024]
//!            [--ctx 16384] [--argmax-out <file>]
//! ```
//!
//! Prefills the first `--prefix` tokens of the sample in batches (that
//! history sits in the KV cache at the chosen precision), then teacher-forces
//! the next `--score` tokens one at a time and reports their mean negative
//! log-likelihood and perplexity. `--argmax-out` writes the model's top-1
//! prediction per scored position, so two precisions can be compared token
//! by token.

use makepad_ai_llm::{LlamaSession, LlamaSessionConfig, TensorType};
use std::io::Write;
use std::time::Instant;

fn main() {
    if let Err(err) = run() {
        eprintln!("kv-quality: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let model = args.next().ok_or("usage: kv-quality <model.gguf> --kv f16|q8 --text <file>")?;
    let (mut kv, mut text, mut prefix, mut score, mut ctx, mut argmax_out) =
        (String::from("f16"), None, 8192usize, 1024usize, 16384u32, None);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--kv" => kv = value,
            "--text" => text = Some(value),
            "--prefix" => prefix = value.parse()?,
            "--score" => score = value.parse()?,
            "--ctx" => ctx = value.parse()?,
            "--argmax-out" => argmax_out = Some(value),
            other => return Err(format!("unknown flag {other}").into()),
        }
    }
    let kv_type = match kv.as_str() {
        "f16" => TensorType::F16,
        "q8" | "q8_0" => TensorType::Q8_0,
        other => return Err(format!("--kv {other}: expected f16 or q8").into()),
    };
    let text = std::fs::read_to_string(text.ok_or("--text is required")?)?;
    let started = Instant::now();
    let mut session = LlamaSession::load(
        &model,
        LlamaSessionConfig {
            max_context: Some(ctx),
            prefill_batch_size: 256,
            attention_k_type: kv_type,
            attention_v_type: kv_type,
            ..LlamaSessionConfig::default()
        },
    )?;
    let load_s = started.elapsed().as_secs_f64();
    let tokens = session.vocab().tokenize(&text, true, false)?;
    if tokens.len() < prefix + score + 1 {
        return Err(format!("sample has {} tokens, need {}", tokens.len(), prefix + score + 1).into());
    }
    let started = Instant::now();
    session.append_tokens(&tokens[..prefix])?;
    let prefill_s = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let mut nll = 0.0f64;
    let mut argmax = Vec::with_capacity(score);
    for i in 0..score {
        let logits = session.last_logits().ok_or("no logits")?;
        let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max) as f64;
        let sum: f64 = logits.iter().map(|l| ((*l as f64) - max).exp()).sum();
        let target = tokens[prefix + i] as usize;
        nll += -((logits[target] as f64 - max) - sum.ln());
        let best = logits
            .iter()
            .enumerate()
            .fold((0usize, f32::NEG_INFINITY), |b, (i, v)| if *v > b.1 { (i, *v) } else { b })
            .0;
        argmax.push(best);
        session.append_token(tokens[prefix + i])?;
    }
    let score_s = started.elapsed().as_secs_f64();
    let mean = nll / score as f64;
    let target_hits = argmax
        .iter()
        .enumerate()
        .filter(|(i, best)| **best == tokens[prefix + i] as usize)
        .count();
    println!(
        "kv={kv} ctx={ctx} prefix={prefix} score={score} nll={mean:.5} ppl={:.4} top1_acc={:.4} \
         load={load_s:.1}s prefill={prefill_s:.1}s ({:.0} tok/s) score={score_s:.1}s",
        mean.exp(),
        target_hits as f64 / score as f64,
        prefix as f64 / prefill_s,
    );
    if let Some(path) = argmax_out {
        let mut out = std::fs::File::create(path)?;
        for best in argmax {
            writeln!(out, "{best}")?;
        }
    }
    Ok(())
}
