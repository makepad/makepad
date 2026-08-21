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

use makepad_ai_llm::{
    LaneEvent, LaneExecutor, LaneOutcome, LaneRequest, LaneScheduler, LlamaModel,
    LlamaSamplingParams, LlamaSession, LlamaSessionConfig, LlamaVocab, SlotTable,
};
use std::collections::HashMap;

/// Speculative draft depth for every session the probe builds. 0 keeps MTP out
/// of the picture entirely; >0 loads the draft head and exercises the
/// hidden-carry ring, which is the structure per-lane MTP repartitions.
static SPEC_DRAFT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn spec_draft() -> usize {
    SPEC_DRAFT.load(std::sync::atomic::Ordering::Relaxed)
}

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
            spec_draft_max: spec_draft(),
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let active = session.speculation_depth();
    if spec_draft() > 0 && active == 0 {
        return Err(format!(
            "--spec {} was requested but the draft head did not load (this gguf has no \n               nextn block). Every speculation assertion below would pass vacuously.",
            spec_draft()
        ));
    }
    if spec_draft() > 0 {
        eprintln!("speculation ACTIVE: draft depth {active}");
    }
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
        eprintln!("usage: llama-slot-probe <model.gguf> [--tokens N] [--slots N] [--timing] [--spec N]");
        std::process::exit(2);
    });
    let mut max_new = 24usize;
    let mut slots = 4u32;
    let mut timing = false;
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
            "--timing" => {
                timing = true;
                index += 1;
            }
            "--spec" => {
                let depth = rest.get(index + 1).and_then(|v| v.parse().ok()).unwrap_or(0);
                SPEC_DRAFT.store(depth, std::sync::atomic::Ordering::Relaxed);
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

    // Fifth gate: DYNAMIC JOIN/LEAVE. Gates 3 and 4 hold the lane set fixed
    // for the whole run. Continuous batching is the claim that lanes may come
    // and go MID-GENERATION without disturbing the conversations already in
    // flight — which is the actual product promise, and nothing above tests it.
    //
    // Both runs follow the SAME width timeline (alone -> 3 wide -> 2 wide at
    // the same steps) and differ only in WHO the neighbours are. Cross-width
    // kernel effects are therefore common-mode and cancel, which matters
    // because spec-on/off stream identity is structurally absent on the Q8_1
    // route: a test that let the width timelines differ would be measuring the
    // quantiser, not the scheduler.
    match dynamic_join_leave(&model, &vocab, slots, PER_SLOT_CONTEXT) {
        Ok(steps) => println!("PASS: lane 0 survived {steps} steps of neighbours joining and leaving"),
        Err(e) => {
            eprintln!("FAIL (dynamic join/leave): {e}");
            std::process::exit(1);
        }
    }

    // Sixth gate: THE SHIPPING CODE PATH. Gates 3-5 drive SlotTable and the
    // session directly — the same call sequence LaneExecutor makes, but not
    // LaneExecutor. This one runs the real LaneScheduler and LaneExecutor that
    // the service will use, so the thing verified on hardware is the thing that
    // ships rather than a faithful imitation of it.
    //
    // Same width-timeline discipline as gate 5: both runs join and leave at the
    // same steps and differ only in WHO the neighbours are.
    match shipping_path(&model, &vocab, slots, PER_SLOT_CONTEXT) {
        Ok(n) => println!("PASS: shipping scheduler kept lane 0 stable over {n} tokens"),
        Err(e) => {
            eprintln!("FAIL (shipping path): {e}");
            std::process::exit(1);
        }
    }

    if timing {
        if let Err(e) = timing_sweep(&model, &vocab, slots, PER_SLOT_CONTEXT) {
            eprintln!("timing failed: {e}");
            std::process::exit(1);
        }
    }
}

/// Drive the REAL LaneScheduler/LaneExecutor through a join/leave timeline and
/// return each job's token stream.
///
/// Greedy sampling (temperature 0) so the streams are deterministic and any
/// difference is the scheduler's doing, not the RNG's.
fn run_shipping(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
    neighbours: &[&str],
) -> Result<HashMap<u64, Vec<i32>>, String> {
    const SOLO_STEPS: usize = 3;
    const WIDE_STEPS: usize = 4;
    const AFTER_LEAVE_STEPS: usize = 3;
    const BUDGET: usize = 64;

    let session = build_session(model, slots, per_slot_context)?;
    let table = session.new_slot_table().map_err(|e| e.to_string())?;
    let scheduler = LaneScheduler::new(table, 8);
    let params = LlamaSamplingParams {
        temperature: 0.0,
        ..LlamaSamplingParams::default()
    };
    // Prove the callback fires: a forgotten publish is the silent failure it
    // exists to prevent, so the gate refuses to pass without evidence it ran.
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen_hits = seen.clone();
    let mut exec = LaneExecutor::new(session, scheduler, params).on_counts(move |_counts| {
        seen_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });

    let mut streams: HashMap<u64, Vec<i32>> = HashMap::new();
    let mut record = |events: Vec<LaneEvent>, streams: &mut HashMap<u64, Vec<i32>>| {
        for event in events {
            if let LaneEvent::Token { job, token, .. } = event {
                streams.entry(job).or_default().push(token);
            }
        }
    };
    let submit = |exec: &mut LaneExecutor, job: u64, text: &str| -> Result<(), String> {
        let prompt = vocab.tokenize(text, true, true).map_err(|e| e.to_string())?;
        exec.scheduler()
            .submit(LaneRequest {
                job,
                session: format!("gate-session-{job}"),
                prompt_tokens: prompt,
                max_new: BUDGET,
            })
            .map_err(|r| format!("submit refused job {}", r.job))
    };
    // A prefill consumes a step, so each lane needs one extra to get going.
    let mut pump = |exec: &mut LaneExecutor,
                    streams: &mut HashMap<u64, Vec<i32>>,
                    decode_steps: usize,
                    prefills: usize|
     -> Result<(), String> {
        for _ in 0..(decode_steps + prefills) {
            record(exec.step()?, streams);
        }
        Ok(())
    };

    submit(&mut exec, 1, LANE_PROMPTS[0])?;
    pump(&mut exec, &mut streams, SOLO_STEPS, 1)?;

    submit(&mut exec, 2, neighbours[0])?;
    submit(&mut exec, 3, neighbours[1])?;
    pump(&mut exec, &mut streams, WIDE_STEPS, 2)?;

    exec.scheduler().cancel(2);
    pump(&mut exec, &mut streams, AFTER_LEAVE_STEPS, 0)?;

    if seen.load(std::sync::atomic::Ordering::Relaxed) == 0 {
        return Err("on_counts never fired — /health would advertise stale occupancy".to_string());
    }
    Ok(streams)
}

fn shipping_path(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
) -> Result<usize, String> {
    if slots < 3 {
        return Err("shipping-path gate needs at least 3 lanes".to_string());
    }
    let a = run_shipping(
        model,
        vocab,
        slots,
        per_slot_context,
        &[LANE_PROMPTS[1], LANE_PROMPTS[2]],
    )?;
    let b = run_shipping(
        model,
        vocab,
        slots,
        per_slot_context,
        &[ALT_PROMPTS[1], ALT_PROMPTS[2]],
    )?;

    let lane_a = a.get(&1).cloned().unwrap_or_default();
    let lane_b = b.get(&1).cloned().unwrap_or_default();
    if lane_a.is_empty() {
        return Err("job 1 produced no tokens through the shipping path".to_string());
    }
    if lane_a != lane_b {
        let at = lane_a
            .iter()
            .zip(&lane_b)
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        return Err(format!(
            "job 1 changed at index {at} when only its NEIGHBOURS changed\n               with A: {lane_a:?}\n  with B: {lane_b:?}"
        ));
    }
    let neigh_a = a.get(&3).cloned().unwrap_or_default();
    let neigh_b = b.get(&3).cloned().unwrap_or_default();
    if neigh_a == neigh_b {
        return Err(format!(
            "neighbour job 3 produced the same stream for both prompt sets, so the \n               experiment did not vary: {neigh_a:?}"
        ));
    }
    println!("  job 1 stable across a join/leave timeline driven by the real scheduler");
    println!("  neighbour job 3 differs between runs (experiment is live)");
    println!("  on_counts fired, so /health would have been kept current");
    Ok(lane_a.len())
}

/// Run lane 0 through a fixed timeline of neighbours joining and leaving,
/// returning lane 0's token stream and the neighbours' streams.
///
/// The timeline is identical between calls; only `neighbours` changes.
fn run_timeline(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
    neighbours: &[&str],
) -> Result<(Vec<i32>, Vec<Vec<i32>>), String> {
    const SOLO_STEPS: usize = 3;
    const WIDE_STEPS: usize = 4;
    const AFTER_LEAVE_STEPS: usize = 3;

    let mut session = build_session(model, slots, per_slot_context)?;
    let mut table = session.new_slot_table().map_err(|e| e.to_string())?;
    let mut next_token = vec![0i32; slots as usize];
    let mut streams: Vec<Vec<i32>> = vec![Vec::new(); slots as usize];

    let mut join = |session: &mut LlamaSession,
                    table: &mut SlotTable,
                    next_token: &mut Vec<i32>,
                    text: &str|
     -> Result<usize, String> {
        let prompt = vocab.tokenize(text, true, true).map_err(|e| e.to_string())?;
        let lane = table.admit().ok_or_else(|| "slot table full".to_string())?;
        let slot = table.slot(lane).ok_or_else(|| "slot missing".to_string())?;
        let (kv_base, state_row) = (slot.kv_base(), slot.live_state_row());
        let logits = session
            .prefill_slot_chunk(lane, kv_base, state_row, 0, &prompt)
            .map_err(|e| format!("lane {lane} prefill: {e}"))?;
        table.advance(lane, prompt.len()).map_err(|e| e.to_string())?;
        table.begin_decoding(lane).map_err(|e| e.to_string())?;
        next_token[lane] = argmax_token_id(&logits).ok_or_else(|| "no argmax".to_string())?;
        Ok(lane)
    };

    let mut decode_steps = |session: &mut LlamaSession,
                            table: &mut SlotTable,
                            next_token: &mut Vec<i32>,
                            streams: &mut Vec<Vec<i32>>,
                            count: usize|
     -> Result<(), String> {
        for _ in 0..count {
            let plan = table.plan_step().ok_or_else(|| "nothing to plan".to_string())?;
            let tokens: Vec<i32> = plan.slots.iter().map(|s| next_token[s.slot]).collect();
            let rows = session
                .step_slots(&plan, &tokens)
                .map_err(|e| format!("step: {e}"))?;
            for (row, step) in rows.iter().zip(&plan.slots) {
                let lane = step.slot;
                streams[lane].push(next_token[lane]);
                next_token[lane] =
                    argmax_token_id(row).ok_or_else(|| "no argmax".to_string())?;
                table.advance(lane, 1).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    };

    join(&mut session, &mut table, &mut next_token, LANE_PROMPTS[0])?;
    decode_steps(&mut session, &mut table, &mut next_token, &mut streams, SOLO_STEPS)?;

    let joined_a = join(&mut session, &mut table, &mut next_token, neighbours[0])?;
    let _joined_b = join(&mut session, &mut table, &mut next_token, neighbours[1])?;
    decode_steps(&mut session, &mut table, &mut next_token, &mut streams, WIDE_STEPS)?;

    table.retire(joined_a).map_err(|e| e.to_string())?;
    decode_steps(
        &mut session,
        &mut table,
        &mut next_token,
        &mut streams,
        AFTER_LEAVE_STEPS,
    )?;

    let lane0 = streams[0].clone();
    Ok((lane0, streams))
}

fn dynamic_join_leave(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
) -> Result<usize, String> {
    if slots < 3 {
        return Err("dynamic join/leave needs at least 3 lanes".to_string());
    }
    let (lane0_a, streams_a) = run_timeline(
        model,
        vocab,
        slots,
        per_slot_context,
        &[LANE_PROMPTS[1], LANE_PROMPTS[2]],
    )?;
    let (lane0_b, streams_b) =
        run_timeline(model, vocab, slots, per_slot_context, &[ALT_PROMPTS[1], ALT_PROMPTS[2]])?;

    if lane0_a != lane0_b {
        let at = lane0_a
            .iter()
            .zip(&lane0_b)
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        return Err(format!(
            "lane 0 changed at index {at} when only its NEIGHBOURS changed\n               with A: {lane0_a:?}\n  with B: {lane0_b:?}\n               identical width timeline, so this is neighbour influence across a join/leave"
        ));
    }
    if streams_a[1] == streams_b[1] {
        return Err(format!(
            "neighbour lane 1 produced the same stream for both prompt sets, so the \n               timeline did not actually vary: {:?}",
            streams_a[1]
        ));
    }
    println!("  lane 0 stable across a 1 -> 3 -> 2 lane timeline with different neighbours");
    println!("  neighbour lane confirmed to differ between runs (experiment is live)");
    Ok(lane0_a.len())
}

/// Steady-state decode rate at 1..N active lanes, from ONE session — which is
/// the dynamic-degradation scenario itself: the same box, the same weights,
/// only the number of lanes actually generating changes.
fn timing_sweep(
    model: &LlamaModel,
    vocab: &LlamaVocab,
    slots: u32,
    per_slot_context: u32,
) -> Result<(), String> {
    const WARMUP: usize = 8;
    const MEASURE: usize = 40;
    let max_lanes = (slots as usize).min(LANE_PROMPTS.len());
    let mut session = build_session(model, slots, per_slot_context)?;

    println!();
    println!("=== steady-state decode, {MEASURE} steps after {WARMUP} warm-up ===");
    println!("{:>6}  {:>11}  {:>13}  {:>11}", "lanes", "step ms", "per-lane tok/s", "aggregate");

    for active in 1..=max_lanes {
        let prompts: Vec<Vec<i32>> = LANE_PROMPTS
            .iter()
            .take(active)
            .map(|t| vocab.tokenize(t, true, true).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let mut table = session.new_slot_table().map_err(|e| e.to_string())?;
        let mut next_token = Vec::with_capacity(active);
        for (lane, prompt) in prompts.iter().enumerate() {
            table.admit().ok_or_else(|| "table full".to_string())?;
            let slot = table.slot(lane).ok_or_else(|| "slot missing".to_string())?;
            let (kv_base, state_row) = (slot.kv_base(), slot.live_state_row());
            let logits = session
                .prefill_slot_chunk(lane, kv_base, state_row, 0, prompt)
                .map_err(|e| format!("lane {lane} prefill: {e}"))?;
            table.advance(lane, prompt.len()).map_err(|e| e.to_string())?;
            table.begin_decoding(lane).map_err(|e| e.to_string())?;
            next_token.push(argmax_token_id(&logits).ok_or_else(|| "no argmax".to_string())?);
        }

        let mut started = None;
        for step in 0..(WARMUP + MEASURE) {
            if step == WARMUP {
                started = Some(std::time::Instant::now());
            }
            let plan = table.plan_step().ok_or_else(|| "nothing to plan".to_string())?;
            let tokens: Vec<i32> = plan.slots.iter().map(|s| next_token[s.slot]).collect();
            let rows = session
                .step_slots(&plan, &tokens)
                .map_err(|e| format!("{active} lanes step {step}: {e}"))?;
            for (row, step_slot) in rows.iter().zip(&plan.slots) {
                let lane = step_slot.slot;
                next_token[lane] = argmax_token_id(row).ok_or_else(|| "no argmax".to_string())?;
                table.advance(lane, 1).map_err(|e| e.to_string())?;
            }
        }
        let elapsed = started.ok_or_else(|| "timer never started".to_string())?.elapsed();
        let step_ms = elapsed.as_secs_f64() * 1e3 / MEASURE as f64;
        let per_lane = 1000.0 / step_ms;
        println!(
            "{:>6}  {:>11.3}  {:>13.1}  {:>11.1}",
            active,
            step_ms,
            per_lane,
            per_lane * active as f64
        );
    }
    println!("(no speculation: one token per lane per step)");
    Ok(())
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
            .prefill_slot_chunk(lane, kv_base, state_row, 0, prompt)
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
            spec_draft_max: spec_draft(),
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
        .prefill_slot_chunk(lane, kv_base, state_row, 0, prompt)
        .map_err(|e| format!("lane {lane} prefill: {e}"))?;
    let mut produced = Vec::new();
    let mut fill = prompt.len();
    for _ in 0..max_new {
        let token = argmax_token_id(&logits).ok_or_else(|| format!("lane {lane}: no argmax"))?;
        produced.push(token);
        logits = session
            .prefill_slot_chunk(lane, kv_base, state_row, fill, &[token])
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
            .prefill_slot_chunk(lane, kv_base, state_row, 0, prompt)
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
            extra_activation_bytes: activation_reserve(slots),
            spec_draft_max: spec_draft(),
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
            .prefill_slot_chunk(slot_index, kv_base, state_row, 0, &prompt)
            .map_err(|e| format!("slot {slot_index} prefill: {e}"))?;

        let mut produced = Vec::new();
        let mut fill = prompt.len();
        for _ in 0..max_new {
            let token = argmax_token_id(&logits)
                .ok_or_else(|| format!("slot {slot_index} produced no argmax"))?;
            produced.push(token);
            logits = session
                .prefill_slot_chunk(slot_index, kv_base, state_row, fill, &[token])
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
