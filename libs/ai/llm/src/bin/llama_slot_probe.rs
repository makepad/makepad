//! Solo-identity gate for multi-slot sessions.
//!
//! The load-bearing promise of the batching work is that **a session built for
//! N slots produces exactly what a single-stream session produces when only one
//! client is talking**. Slots share one flat attention arena and the slot index
//! is folded into the context dimension, so widening a session from 1 slot to N
//! makes the cache tensors N times taller and changes nothing else: slot 0's
//! base is 0, it writes rows `0..fill`, and its mask lower bound is 0.
//!
//! This gate proves that end to end on a real model. If it ever fails, the
//! folding has stopped being output-neutral and no batching number is
//! trustworthy until it passes again.
//!
//! Usage: `llama-slot-probe <model.gguf> [--tokens N] [--slots N]`

use makepad_ai_llm::{LlamaModel, LlamaSession, LlamaSessionConfig, LlamaVocab};

/// First index of the maximum, tie-broken low — llama.cpp's greedy ordering.
fn argmax_token_id(logits: &[f32]) -> Option<i32> {
    let mut best: Option<(usize, f32)> = None;
    for (index, &value) in logits.iter().enumerate() {
        if best.map(|(_, b)| value > b).unwrap_or(true) {
            best = Some((index, value));
        }
    }
    best.map(|(index, _)| index as i32)
}

const PROMPT: &str = "Explain in two sentences why a memory-bound decode step \
gets cheaper per token when several sequences are batched together.";

fn generate(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    max_new: usize,
    max_context: u32,
) -> Result<(Vec<i32>, Vec<f32>, usize), String> {
    let mut session = LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(max_context),
            max_sequences: slots,
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let arena = session.attention_arena_rows();
    let tokens = vocab
        .tokenize(PROMPT, true, true)
        .map_err(|e| e.to_string())?;
    session.append_tokens(&tokens).map_err(|e| e.to_string())?;
    let generation = session
        .continue_greedy(max_new)
        .map_err(|e| e.to_string())?;
    let logits = session
        .last_logits()
        .map(|l| l.to_vec())
        .unwrap_or_default();
    Ok((generation.token_ids, logits, arena))
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| {
        eprintln!("usage: llama-slot-probe <model.gguf> [--tokens N] [--slots N]");
        std::process::exit(2);
    });
    let mut max_new = 24usize;
    let mut slots = 4u32;
    let mut rest: Vec<String> = args.collect();
    let mut index = 0;
    while index < rest.len() {
        match rest[index].as_str() {
            "--tokens" => {
                max_new = rest
                    .get(index + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(max_new);
                index += 2;
            }
            "--slots" => {
                slots = rest
                    .get(index + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(slots);
                index += 2;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        let _ = &mut rest;
    }

    // Per-slot context, so the N-slot arena is N times this. Kept small so the
    // gate is quick; the property under test does not depend on its size.
    const PER_SLOT_CONTEXT: u32 = 2048;

    let model = LlamaModel::load(std::path::Path::new(&path)).unwrap_or_else(|e| {
        eprintln!("load {path}: {e}");
        std::process::exit(1);
    });
    let vocab = LlamaVocab::from_model(&model).unwrap_or_else(|e| {
        eprintln!("vocab: {e}");
        std::process::exit(1);
    });

    eprintln!("solo reference: 1 slot, {PER_SLOT_CONTEXT} context");
    let (solo_tokens, solo_logits, solo_arena) =
        generate(&model, &vocab, 1, max_new, PER_SLOT_CONTEXT).unwrap_or_else(|e| {
            eprintln!("solo run: {e}");
            std::process::exit(1);
        });

    eprintln!("widened: {slots} slots, {PER_SLOT_CONTEXT} context per slot");
    let (wide_tokens, wide_logits, wide_arena) =
        generate(&model, &vocab, slots, max_new, PER_SLOT_CONTEXT).unwrap_or_else(|e| {
            eprintln!("widened run: {e}");
            std::process::exit(1);
        });

    println!("arena rows: solo={solo_arena} widened={wide_arena}");
    if wide_arena != solo_arena * slots as usize {
        eprintln!(
            "FAIL: a {slots}-slot arena should be {} rows, got {wide_arena}",
            solo_arena * slots as usize
        );
        std::process::exit(1);
    }

    println!("solo   tokens: {solo_tokens:?}");
    println!("widened tokens: {wide_tokens:?}");

    if solo_tokens != wide_tokens {
        let first = solo_tokens
            .iter()
            .zip(&wide_tokens)
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| solo_tokens.len().min(wide_tokens.len()));
        eprintln!("FAIL: token streams diverge at index {first}");
        eprintln!("  a widened session must not change what a solo client sees");
        std::process::exit(1);
    }

    let max_abs = solo_logits
        .iter()
        .zip(&wide_logits)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    println!("last-logits max abs diff: {max_abs:e}");
    if max_abs != 0.0 {
        // Same kernels, same shapes, same reduction order — the only change is
        // how tall the cache tensor is. Anything nonzero means the fold is
        // touching values, not just addressing.
        eprintln!("FAIL: widening the arena perturbed the logits (expected exactly 0)");
        std::process::exit(1);
    }

    println!("PASS: {slots} slots produce byte-identical solo output");

    // Second gate: SLOT INDEPENDENCE. A conversation living in slot k must
    // produce exactly what it produces in slot 0. This is the one that
    // exercises the slot-major addressing itself — kv_base on every cache
    // write, the mask lower bound, absolute row spans — rather than just the
    // arena being taller. Runs at n_seqs == 1 per chunk, so it works on Metal.
    match slot_independence(&model, &vocab, slots, max_new, PER_SLOT_CONTEXT) {
        Ok(()) => println!("PASS: every slot decodes what slot 0 decodes"),
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}

/// Prefill the same prompt into each slot in turn and greedily decode it there,
/// asserting every slot yields slot 0's token stream.
fn slot_independence(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    max_new: usize,
    per_slot_context: u32,
) -> Result<(), String> {
    let mut session = LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(per_slot_context),
            max_sequences: slots,
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let prompt = vocab
        .tokenize(PROMPT, true, true)
        .map_err(|e| e.to_string())?;

    let mut reference: Option<Vec<i32>> = None;
    for slot_index in 0..slots as usize {
        let table = session.new_slot_table().map_err(|e| e.to_string())?;
        let slot = table
            .slot(slot_index)
            .ok_or_else(|| format!("slot {slot_index} missing from table"))?;
        let kv_base = slot.kv_base();
        let state_row = slot.live_state_row();

        let mut logits = session
            .prefill_slot_chunk(kv_base, state_row, 0, &prompt)
            .map_err(|e| format!("slot {slot_index} prefill: {e}"))?;

        let mut produced = Vec::new();
        let mut fill = prompt.len();
        for _ in 0..max_new {
            let token = argmax_token_id(&logits)
                .ok_or_else(|| format!("slot {slot_index} produced no argmax"))?;
            produced.push(token);
            logits = session
                .prefill_slot_chunk(kv_base, state_row, fill, &[token])
                .map_err(|e| format!("slot {slot_index} decode: {e}"))?;
            fill += 1;
        }

        match &reference {
            None => {
                println!("slot 0 tokens: {produced:?}");
                reference = Some(produced);
            }
            Some(expected) => {
                if *expected != produced {
                    let at = expected
                        .iter()
                        .zip(&produced)
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    return Err(format!(
                        "slot {slot_index} diverges from slot 0 at index {at}\n  slot 0: {expected:?}\n  slot {slot_index}: {produced:?}"
                    ));
                }
                println!("slot {slot_index}: identical to slot 0");
            }
        }
    }
    Ok(())
}
