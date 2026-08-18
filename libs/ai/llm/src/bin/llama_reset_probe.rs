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

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let model_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "local/models/Qwen3.5-4B-Q5_K_M.gguf".to_string());
    let config = LlamaSessionConfig {
        max_context: Some(2048),
        ..LlamaSessionConfig::default()
    };
    let mut session = LlamaSession::load(&model_path, config)?;
    let vocab = session.vocab().clone();

    let mut failures = 0;

    let w0 = session.debug_weights_fingerprint(16)?;
    println!("weights fingerprint cold:        {w0:016x}");

    // 1. chat -> reset -> chat: identical streams required.
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
