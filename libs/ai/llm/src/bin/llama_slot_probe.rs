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

use makepad_ai_llm::{LlamaModel, LlamaSession, LlamaSessionConfig, LlamaVocab, SlotTable};

/// Graph activations grow with the slot count: wider batches mean wider
/// intermediates, and the arena the attention mask spans grows too. The 512 MiB
/// default is sized for one sequence, and at 8 slots it is not enough — the
/// session dies allocating a graph tensor before it can run anything.
fn activation_reserve(slots: u32) -> usize {
    (512 + 192 * slots.max(1) as usize) << 20
}

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
            extra_activation_bytes: activation_reserve(slots),
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

    // Third gate: BATCHED INTERLEAVE. Everything above runs at n_seqs == 1.
    // This one decodes several lanes IN ONE STEP and demands each lane produce
    // exactly what it produced when stepped alone. If any lane's state or KV
    // leaks into another, a stream changes — and cross-lane bleed is the
    // failure that reads as a model-quality problem rather than an error.
    // CUDA only: Metal refuses n_seqs > 1 by design.
    match batched_interleave(&model, &vocab, slots, max_new, PER_SLOT_CONTEXT) {
        Ok(lanes) => println!("PASS: {lanes} lanes batched, streams match stepped-alone"),
        Err(e) => {
            eprintln!("FAIL (cross-width): {e}");
            eprintln!("  NOTE: this gate compares batch width B against width 1, and the");
            eprintln!("  shipped Q8_1 mmvq route is NOT bit-identical across widths.");
            eprintln!("  Re-run with MKLLM_DISABLE_Q81_MMVQ=1 BEFORE concluding cross-lane");
            eprintln!("  bleed. If it passes there, this is the width finding, not batching.");
            eprintln!("  Gate 4 is width-invariant and does not have this ambiguity.");
            std::process::exit(1);
        }
    }

    // Fourth gate: WIDTH-INVARIANT BLEED. Gate 3 compares a lane at batch
    // width B against the same lane at width 1, so a divergence there can come
    // from cross-width kernel numerics rather than from batching. This one
    // holds the width CONSTANT and changes only WHO the neighbours are: lane 0
    // runs the same prompt in two B-wide batches whose other lanes carry
    // different work. Same width, same kernel path, same reduction order — so
    // any difference in lane 0's stream is neighbour influence and nothing
    // else. This is the gate that actually defines cross-lane bleed.
    match neighbour_invariance(&model, &vocab, slots, max_new, PER_SLOT_CONTEXT) {
        Ok(lanes) => println!("PASS: lane 0 unchanged by its {lanes} neighbours (width-invariant)"),
        Err(e) => {
            eprintln!("FAIL (cross-lane bleed): {e}");
            std::process::exit(1);
        }
    }
}

/// A second, disjoint set of neighbour prompts. Different lengths AND different
/// content from `LANE_PROMPTS`, so swapping them changes every neighbour's fill
/// and token stream.
const ALT_PROMPTS: &[&str] = &[
    "Recite the first four prime numbers.",
    "In one long paragraph, describe how a garden changes across the four seasons, mentioning at least one plant per season and the weather that drives the change.",
    "Translate to French: good morning.",
    "What colour is the sky at noon on a clear day, and why?",
    "Count backwards from ten to one.",
    "Give the capital of Japan.",
    "Explain gravity to a six year old.",
    "Name two kinds of cloud.",
];

/// Decode `lanes` lanes together for `max_new` steps and return every lane's
/// stream. Lane 0 always gets `LANE_PROMPTS[0]`; the others come from `others`.
fn run_batch_with_neighbours(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    lanes: usize,
    max_new: usize,
    per_slot_context: u32,
    others: &[&str],
) -> Result<Vec<Vec<i32>>, String> {
    let mut prompts = vec![vocab
        .tokenize(LANE_PROMPTS[0], true, true)
        .map_err(|e| e.to_string())?];
    for text in others.iter().take(lanes - 1) {
        prompts.push(vocab.tokenize(text, true, true).map_err(|e| e.to_string())?);
    }

    let mut session = build_session(model, slots, per_slot_context)?;
    let mut table = session.new_slot_table().map_err(|e| e.to_string())?;
    let mut next_token = Vec::with_capacity(lanes);
    for (lane, prompt) in prompts.iter().enumerate() {
        let claimed = table.admit().ok_or_else(|| "slot table full".to_string())?;
        if claimed != lane {
            return Err(format!("expected slot {lane}, got {claimed}"));
        }
        let slot = table.slot(lane).ok_or_else(|| "slot missing".to_string())?;
        let (kv_base, state_row) = (slot.kv_base(), slot.live_state_row());
        let logits = session
            .prefill_slot_chunk(kv_base, state_row, 0, prompt)
            .map_err(|e| format!("lane {lane} prefill: {e}"))?;
        table
            .advance(lane, prompt.len())
            .map_err(|e| e.to_string())?;
        table.begin_decoding(lane).map_err(|e| e.to_string())?;
        next_token.push(argmax_token_id(&logits).ok_or_else(|| "no argmax".to_string())?);
    }

    let mut produced: Vec<Vec<i32>> = vec![Vec::new(); lanes];
    for step in 0..max_new {
        let plan = table
            .plan_step()
            .ok_or_else(|| format!("step {step}: nothing to plan"))?;
        let tokens: Vec<i32> = plan.slots.iter().map(|s| next_token[s.slot]).collect();
        let rows = session
            .step_slots(&plan, &tokens)
            .map_err(|e| format!("step {step}: {e}"))?;
        for (row, step_slot) in rows.iter().zip(&plan.slots) {
            let lane = step_slot.slot;
            produced[lane].push(next_token[lane]);
            next_token[lane] =
                argmax_token_id(row).ok_or_else(|| format!("lane {lane}: no argmax"))?;
            table.advance(lane, 1).map_err(|e| e.to_string())?;
        }
    }
    Ok(produced)
}

fn neighbour_invariance(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    max_new: usize,
    per_slot_context: u32,
) -> Result<usize, String> {
    let lanes = (slots as usize).min(LANE_PROMPTS.len()).min(ALT_PROMPTS.len());
    if lanes < 2 {
        return Err("neighbour invariance needs at least 2 lanes".to_string());
    }
    let with_a = run_batch_with_neighbours(
        model,
        vocab,
        slots,
        lanes,
        max_new,
        per_slot_context,
        &LANE_PROMPTS[1..],
    )?;
    let with_b = run_batch_with_neighbours(
        model,
        vocab,
        slots,
        lanes,
        max_new,
        per_slot_context,
        ALT_PROMPTS,
    )?;

    if with_a[0] != with_b[0] {
        let at = with_a[0]
            .iter()
            .zip(&with_b[0])
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        return Err(format!(
            "lane 0 changed when only its NEIGHBOURS changed, at index {at}\n               neighbours A: {:?}\n  neighbours B: {:?}\n               same batch width, same kernels — this is neighbour influence",
            with_a[0], with_b[0]
        ));
    }
    // Sanity: the neighbours really did diverge, or the test proved nothing.
    if lanes > 1 && with_a[1] == with_b[1] {
        return Err(format!(
            "neighbour lane 1 produced the SAME stream for both prompt sets, so the \n               experiment did not actually vary anything: {:?}",
            with_a[1]
        ));
    }
    println!("  lane 0 stream stable across two different neighbour sets");
    println!("  lane 1 confirmed to differ between sets (experiment is live)");
    Ok(lanes - 1)
}

/// Distinct prompts, deliberately of DIFFERENT lengths, so lanes sit at
/// different fills and the per-lane position and key-span handling is exercised
/// rather than every lane moving in lockstep.
const LANE_PROMPTS: &[&str] = &[
    "List three primary colours.",
    "Explain in one sentence what a KV cache is and why decoding needs one.",
    "Name a river in France.",
    "Write a short definition of throughput as distinct from latency, then give one example of a system that optimises for each.",
    "What is 17 plus 4?",
    "Describe what a memory-bound kernel is.",
    "Give one reason batching helps GPU inference.",
    "Say hello.",
];

fn build_session(
    model: &LlamaModel,
    slots: u32,
    per_slot_context: u32,
) -> Result<LlamaSession, String> {
    LlamaSession::from_model(
        model,
        LlamaSessionConfig {
            max_context: Some(per_slot_context),
            max_sequences: slots,
            extra_activation_bytes: activation_reserve(slots),
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// Decode `lane` alone, one token per step at n_seqs == 1 — the already-gated
/// path, used here as the reference the batched run must reproduce.
fn decode_lane_alone(
    session: &mut LlamaSession,
    table: &SlotTable,
    lane: usize,
    prompt: &[i32],
    max_new: usize,
) -> Result<Vec<i32>, String> {
    let slot = table
        .slot(lane)
        .ok_or_else(|| format!("slot {lane} missing"))?;
    let (kv_base, state_row) = (slot.kv_base(), slot.live_state_row());
    let mut logits = session
        .prefill_slot_chunk(kv_base, state_row, 0, prompt)
        .map_err(|e| format!("lane {lane} prefill: {e}"))?;
    let mut produced = Vec::new();
    let mut fill = prompt.len();
    for _ in 0..max_new {
        let token = argmax_token_id(&logits).ok_or_else(|| format!("lane {lane}: no argmax"))?;
        produced.push(token);
        logits = session
            .prefill_slot_chunk(kv_base, state_row, fill, &[token])
            .map_err(|e| format!("lane {lane} decode: {e}"))?;
        fill += 1;
    }
    Ok(produced)
}

fn batched_interleave(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    max_new: usize,
    per_slot_context: u32,
) -> Result<usize, String> {
    let lanes = (slots as usize).min(LANE_PROMPTS.len());
    if lanes < 2 {
        return Err("batched interleave needs at least 2 lanes".to_string());
    }
    let prompts: Vec<Vec<i32>> = LANE_PROMPTS
        .iter()
        .take(lanes)
        .map(|text| vocab.tokenize(text, true, true).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    for (lane, prompt) in prompts.iter().enumerate() {
        println!("  lane {lane}: {} prompt tokens", prompt.len());
    }

    // Reference pass: each lane stepped ALONE in its own slot.
    let mut session = build_session(model, slots, per_slot_context)?;
    let table = session.new_slot_table().map_err(|e| e.to_string())?;
    let mut reference = Vec::new();
    for (lane, prompt) in prompts.iter().enumerate() {
        reference.push(decode_lane_alone(&mut session, &table, lane, prompt, max_new)?);
    }

    // Batched pass: a fresh session so no state carries over, all lanes
    // prefilled, then decoded TOGETHER one step at a time.
    //
    // Drop the reference session FIRST. Shadowing it would keep it alive to the
    // end of the block, and each session owns its own device copy of the
    // weights — two live 15 GB sessions do not fit on a 32 GB card, and the
    // failure would be an OOM in the gate rather than anything about batching.
    drop(session);
    drop(table);
    let mut session = build_session(model, slots, per_slot_context)?;
    let mut table = session.new_slot_table().map_err(|e| e.to_string())?;
    let mut next_token = Vec::with_capacity(lanes);
    for (lane, prompt) in prompts.iter().enumerate() {
        let claimed = table.admit().ok_or_else(|| "slot table full".to_string())?;
        if claimed != lane {
            return Err(format!("expected to claim slot {lane}, got {claimed}"));
        }
        let slot = table.slot(lane).ok_or_else(|| "slot missing".to_string())?;
        let (kv_base, state_row) = (slot.kv_base(), slot.live_state_row());
        let logits = session
            .prefill_slot_chunk(kv_base, state_row, 0, prompt)
            .map_err(|e| format!("lane {lane} batched prefill: {e}"))?;
        table
            .advance(lane, prompt.len())
            .map_err(|e| e.to_string())?;
        table.begin_decoding(lane).map_err(|e| e.to_string())?;
        next_token.push(argmax_token_id(&logits).ok_or_else(|| "no argmax".to_string())?);
    }

    let mut produced: Vec<Vec<i32>> = vec![Vec::new(); lanes];
    for step in 0..max_new {
        let plan = table
            .plan_step()
            .ok_or_else(|| format!("step {step}: nothing to plan"))?;
        if plan.slots.len() != lanes {
            return Err(format!(
                "step {step}: expected {lanes} lanes in the batch, got {}",
                plan.slots.len()
            ));
        }
        let tokens: Vec<i32> = plan.slots.iter().map(|s| next_token[s.slot]).collect();
        let rows = session
            .step_slots(&plan, &tokens)
            .map_err(|e| format!("step {step}: {e}"))?;
        if rows.len() != lanes {
            return Err(format!(
                "step {step}: expected {lanes} logit rows, got {}",
                rows.len()
            ));
        }
        for (row, step_slot) in rows.iter().zip(&plan.slots) {
            let lane = step_slot.slot;
            produced[lane].push(next_token[lane]);
            next_token[lane] =
                argmax_token_id(row).ok_or_else(|| format!("lane {lane}: no argmax"))?;
            table.advance(lane, 1).map_err(|e| e.to_string())?;
        }
    }

    let mut failures = Vec::new();
    for lane in 0..lanes {
        let expected = &reference[lane][..produced[lane].len()];
        if expected != produced[lane].as_slice() {
            let at = expected
                .iter()
                .zip(&produced[lane])
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            failures.push(format!(
                "lane {lane} diverges at index {at}\n    alone:   {expected:?}\n    batched: {:?}",
                produced[lane]
            ));
        } else {
            println!("  lane {lane}: batched stream identical to stepped-alone");
        }
    }
    if !failures.is_empty() {
        return Err(format!(
            "cross-lane bleed in a {lanes}-lane batch:\n  {}",
            failures.join("\n  ")
        ));
    }
    Ok(lanes)
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
