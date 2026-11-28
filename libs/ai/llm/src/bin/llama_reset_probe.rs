//! Reset-determinism probe: prefill+decode a prompt, `reset()`, run the
//! identical prompt again, and diff the greedy token streams. A clean reset
//! must reproduce the first stream exactly; any divergence means session
//! state survives reset (the asset-ai expand→chat empty-reply bug).
//!
//!     cargo run --release -p makepad-ai-llm --bin llama-reset-probe -- \
//!         local/models/Qwen3.5-4B-Q5_K_M.gguf

use makepad_ai_llm::{LlamaSession, LlamaSessionConfig, LlamaVocab};

const CHAT_PROMPT: &str = "<|im_start|>system\nReply with one short sentence.<|im_end|>\n<|im_start|>user\nSay ready.<|im_end|>\n<|im_start|>assistant\n<think>\n";
const LONG_PROMPT: &str = "<|im_start|>system\nTarget domain: image.\n\nYou expand terse asset intents into rich, single-paragraph visual descriptions for an image generator. Describe subject, materials, lighting, mood, camera framing, and background in concrete visual language. Never mention the prompt, the model, or these instructions.<|im_end|>\n<|im_start|>user\nIntent: a red fox<|im_end|>\n<|im_start|>assistant\n<think>\n";

fn top_k(logits: &[f32], k: usize) -> Vec<(i32, f32)> {
    let mut idx: Vec<usize> = (0..logits.len()).collect();
    idx.sort_unstable_by(|a, b| logits[*b].partial_cmp(&logits[*a]).unwrap());
    idx.iter()
        .take(k)
        .map(|i| (*i as i32, logits[*i]))
        .collect()
}

fn decode_run(
    session: &mut LlamaSession,
    vocab: &LlamaVocab,
    prompt: &str,
    max_new: usize,
    label: &str,
) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
    let mut tokens = vocab.tokenize(prompt, true, true)?;
    if tokens.last().copied() == vocab.eos_token_id() {
        tokens.pop();
    }
    session.append_tokens(&tokens)?;
    if let Some(logits) = session.last_logits() {
        println!("{label} prefill top5: {:?}", top_k(logits, 5));
    }
    let mut out = Vec::new();
    for _ in 0..max_new {
        match session.next_greedy_token()? {
            Some(tok) => out.push(tok),
            None => break,
        }
    }
    Ok(out)
}

/// Compare two continuations of the same generated KV with a cold ChatML prefill.
fn warm_probe(
    session: &mut LlamaSession,
    vocab: &LlamaVocab,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};

    const FIRST: &str = "<|im_start|>system\nAnswer directly.<|im_end|>\n<|im_start|>user\nWhat is the capital of Japan?<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
    const CLOSE: &str = "<|im_end|>\n";
    const NEXT: &str = "<|im_start|>user\nCount from one to twenty in words.<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
    const M: usize = 8;
    const ROUNDS: usize = 7;

    // Match the worker's final text segment, both for cold and delta prefills.
    let tokenize = |text: &str, first: bool| -> makepad_ai_llm::Result<Vec<i32>> {
        let mut ids = vocab.tokenize(text, first, true)?;
        if ids.last().copied() == vocab.eos_token_id() {
            ids.pop();
        }
        Ok(ids)
    };
    let first = tokenize(FIRST, true)?;
    session.append_tokens(&first)?;
    let reply = session.continue_greedy(24)?;
    let reply_span = first.len()..session.token_count();
    println!(
        "turn one: {reply_span:?} reply tokens, {:?}, {:?}",
        reply.stop_reason, reply.text
    );
    let checkpoint = session.save_state()?;
    let cold = tokenize(&format!("{FIRST}{}{CLOSE}{NEXT}", reply.text), true)?;
    let deltas = [
        tokenize(&format!("{CLOSE}{NEXT}"), false)?,
        tokenize(NEXT, false)?,
    ];
    let labels = ["after", "before"];

    // Collect logits outside the timing series. Include an end token's
    // prediction and margin even when the session stops before appending it.
    let trace = |session: &mut LlamaSession| -> Result<Vec<(Option<i32>, f32)>, Box<dyn std::error::Error>> {
        let mut steps = Vec::new();
        for _ in 0..M {
            let best = top_k(session.last_logits().ok_or("missing logits after prefill")?, 2);
            let token = session.next_greedy_token()?;
            steps.push((token, best[0].1 - best[1].1));
            if token.is_none() {
                break;
            }
        }
        Ok(steps)
    };
    session.reset()?;
    session.append_tokens(&cold)?;
    let reference = trace(session)?;
    for (label, delta) in labels.iter().zip(&deltas) {
        session.restore_state(&checkpoint)?;
        session.append_tokens(delta)?;
        let ids = session.token_ids();
        let common = ids.iter().zip(&cold).take_while(|(a, b)| a == b).count();
        if ids == cold {
            println!(
                "{label} KV matches all {} cold ids ({} delta tokens)",
                cold.len(),
                delta.len()
            );
        } else {
            println!("{label} KV first difference at {common}, in reply span: {}, ids {:?} vs cold {:?} (lengths {} vs {})",
                reply_span.contains(&common), ids.get(common), cold.get(common), ids.len(), cold.len());
        }
        let steps = trace(session)?;
        if let Some((i, ((id, margin), (cold_id, cold_margin)))) = steps
            .iter()
            .zip(&reference)
            .enumerate()
            .find(|(_, (a, b))| a.0 != b.0)
        {
            println!("{label} greedy first difference at {i}: id {id:?}, margin {margin:.6}; cold id {cold_id:?}, margin {cold_margin:.6}");
        } else {
            assert_eq!(
                steps.len(),
                reference.len(),
                "identical predictions must stop at the same token"
            );
            println!(
                "{label} greedy matches all {} cold predictions",
                steps.len()
            );
        }
    }

    let mut timings = [Vec::new(), Vec::new()];
    for round in 0..=ROUNDS {
        // Round zero warms both paths. Alternate which is first thereafter.
        for path in [round % 2, 1 - round % 2] {
            session.restore_state(&checkpoint)?;
            let start = Instant::now();
            session.append_tokens(&deltas[path])?;
            session
                .next_greedy_token()?
                .ok_or("reply ended before the first timed token")?;
            let first = start.elapsed();
            for _ in 1..M {
                session
                    .next_greedy_token()?
                    .ok_or("reply ended before M timed tokens")?;
            }
            let total = start.elapsed();
            if round > 0 {
                timings[path].push((first, total));
            }
        }
    }
    let median_ms = |mut samples: Vec<Duration>| {
        samples.sort_unstable();
        samples[samples.len() / 2].as_secs_f64() * 1000.0
    };
    for (label, samples) in labels.iter().zip(&timings) {
        println!("{label} medians over {ROUNDS} rounds: delta + first {:.3} ms, delta + {M} tokens {:.3} ms",
            median_ms(samples.iter().map(|s| s.0).collect()),
            median_ms(samples.iter().map(|s| s.1).collect()));
    }
    Ok(())
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let model_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "local/models/Qwen3.5-4B-Q5_K_M.gguf".to_string());
    let mode = std::env::args().nth(2).unwrap_or_default();
    let config = LlamaSessionConfig {
        max_context: Some(2048),
        ..LlamaSessionConfig::default()
    };
    let mut session = LlamaSession::load(&model_path, config)?;
    let vocab = session.vocab().clone();

    if mode == "warm" {
        return warm_probe(&mut session, &vocab);
    }

    // `once`: single prefill + short decode, then exit — the minimal target
    // for compute-sanitizer racecheck/initcheck runs.
    if mode == "once" {
        let toks = decode_run(&mut session, &vocab, CHAT_PROMPT, 4, "once")?;
        println!("once ({} tok): {:?}", toks.len(), &toks);
        return Ok(());
    }
    // `localize`: two identical prefills with a snapshot of every cache
    // tensor after each; the earliest layer whose K/V or r/s rows differ is
    // where the forward pass diverges first.
    if mode == "localize" {
        let mut tokens = vocab.tokenize(CHAT_PROMPT, true, true)?;
        if tokens.last().copied() == vocab.eos_token_id() {
            tokens.pop();
        }
        session.reset()?;
        session.append_tokens(&tokens)?;
        let s1 = session.debug_state_snapshot()?;
        session.reset()?;
        session.append_tokens(&tokens)?;
        let s2 = session.debug_state_snapshot()?;
        let mut diffs = 0;
        for ((n1, off, len, h1), (_, _, _, h2)) in s1.iter().zip(s2.iter()) {
            if h1 != h2 {
                diffs += 1;
                println!("DIVERGE {n1} (offset {off} len {len})");
            }
        }
        println!("{diffs} cache tensors diverge between identical prefills (of {})", s1.len());
        return Ok(());
    }
    // `hammer N`: N times (reset + prefill + 1 greedy step), count distinct
    // first-token outcomes — quantifies the nondeterminism incidence.
    if mode == "hammer" {
        let n: usize = std::env::args()
            .nth(3)
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        let mut outcomes: std::collections::BTreeMap<String, usize> = Default::default();
        for _ in 0..n {
            session.reset()?;
            let toks = decode_run(&mut session, &vocab, CHAT_PROMPT, 1, "hammer")?;
            *outcomes.entry(format!("{toks:?}")).or_default() += 1;
        }
        println!("hammer outcomes over {n} runs:");
        for (outcome, count) in &outcomes {
            println!("  {count}x {outcome}");
        }
        println!(
            "{}",
            if outcomes.len() == 1 {
                "PASS hammer deterministic"
            } else {
                "FAIL hammer nondeterministic"
            }
        );
        return Ok(());
    }

    let mut failures = 0;

    let w0 = session.debug_weights_fingerprint(16)?;
    println!("weights fingerprint cold:        {w0:016x}");

    // 1. chat -> plain reset -> chat: identical streams required.
    let a = decode_run(&mut session, &vocab, CHAT_PROMPT, 32, "chat A")?;
    let w1 = session.debug_weights_fingerprint(16)?;
    println!("weights fingerprint after chatA: {w1:016x} (changed={})", w1 != w0);
    session.reset()?;
    let b = decode_run(&mut session, &vocab, CHAT_PROMPT, 32, "chat B")?;
    println!("chat A ({} tok): {:?}", a.len(), &a);
    println!("chat B ({} tok): {:?}", b.len(), &b);
    if a == b {
        println!("PASS chat->reset->chat deterministic");
    } else {
        failures += 1;
        println!("FAIL chat->reset->chat DIVERGED");
    }

    // 1b. chat -> SCORCHED reset (zero every non-weight tensor) -> chat.
    let cleared = session.debug_scorched_reset()?;
    println!("scorched reset cleared {cleared} non-weight tensor ranges");
    let sb = decode_run(&mut session, &vocab, CHAT_PROMPT, 32, "chat SB")?;
    println!("chat SB ({} tok): {:?}", sb.len(), &sb);
    if a == sb {
        println!("PASS chat->scorched->chat matches cold run");
    } else {
        failures += 1;
        println!("FAIL chat->scorched->chat STILL DIVERGED from cold");
    }

    // 2. long expand -> reset -> chat: must still match the clean chat run.
    session.reset()?;
    let e = decode_run(&mut session, &vocab, LONG_PROMPT, 128, "expand1")?;
    println!("expand1 ({} tok) text: {:?}", e.len(), vocab.decode_tokens(&e)?);
    let w2 = session.debug_weights_fingerprint(16)?;
    println!("weights fingerprint after expand: {w2:016x} (changed={})", w2 != w0);
    session.reset()?;
    let c = decode_run(&mut session, &vocab, CHAT_PROMPT, 32, "chat C")?;
    println!("chat C ({} tok): {:?}", c.len(), &c);
    println!("  C text: {:?}", vocab.decode_tokens(&c)?);
    if a == c {
        println!("PASS expand->reset->chat deterministic");
    } else {
        failures += 1;
        println!("FAIL expand->reset->chat DIVERGED");
    }

    // 3. expand -> reset -> expand: does the long prompt reproduce itself?
    session.reset()?;
    let e2 = decode_run(&mut session, &vocab, LONG_PROMPT, 128, "expand2")?;
    if e == e2 {
        println!("PASS expand->reset->expand deterministic");
    } else {
        failures += 1;
        println!("FAIL expand->reset->expand DIVERGED");
        println!("expand2 ({} tok) text: {:?}", e2.len(), vocab.decode_tokens(&e2)?);
    }

    // 4. Name the leaking buffers: snapshot every tensor right after two
    // different resets. Anything that differs carries state across reset.
    session.reset()?;
    let s1 = session.debug_state_snapshot()?;
    let _ = decode_run(&mut session, &vocab, CHAT_PROMPT, 8, "leak-probe")?;
    session.reset()?;
    let s2 = session.debug_state_snapshot()?;
    let mut diffs = 0;
    for ((n1, off, len, h1), (_, _, _, h2)) in s1.iter().zip(s2.iter()) {
        if h1 != h2 {
            diffs += 1;
            if diffs <= 60 {
                println!("LEAK {n1} at {off} len {len}");
            }
        }
    }
    println!("{diffs} tensors differ across reset (of {})", s1.len());

    if failures == 0 {
        println!("ALL GREEN");
        Ok(())
    } else {
        Err(format!("{failures} determinism failures").into())
    }
}

fn main() {
    match run() {
        Ok(()) => {}
        Err(err) => {
            eprintln!("llama-reset-probe failed: {err}");
            std::process::exit(1);
        }
    }
}
