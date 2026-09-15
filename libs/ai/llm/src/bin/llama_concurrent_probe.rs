//! Reproduce the route-app shape: two LlamaSessions (dispatcher + gate) in
//! ONE process, generating concurrently on their own threads. Compares the
//! big model's tokens against a solo-run reference to detect cross-session
//! interference while sharing the mmap-backed weight path.

use makepad_ai_llm::{LlamaModel, LlamaSession, LlamaSessionConfig, LlamaVocab};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

const BIG_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";
const SMALL_MODEL: &str = "local/models/Qwen3.5-4B-Q5_K_M.gguf";
const BIG_PROMPT: &str = "You are a trip planner. The user asks: plan a route \
from Amsterdam to Utrecht with a charging stop. Reply briefly.";
const SMALL_PROMPT: &str = "Classify: is 'hi computer show me the map' directed \
at the assistant? Answer yes or no.";
const MAX_NEW: usize = 24;
const MAX_CONTEXT: u32 = 8192;

fn run_big_once() -> Result<Vec<i32>, String> {
    let model = LlamaModel::load(std::path::Path::new(BIG_MODEL)).map_err(|e| e.to_string())?;
    let vocab = LlamaVocab::from_model(&model).map_err(|e| e.to_string())?;
    let mut session = LlamaSession::from_model(
        &model,
        LlamaSessionConfig {
            max_context: Some(MAX_CONTEXT),
            ..LlamaSessionConfig::default()
        },
    )
    .map_err(|e| e.to_string())?;
    let tokens = vocab
        .tokenize(BIG_PROMPT, true, true)
        .map_err(|e| e.to_string())?;
    session.append_tokens(&tokens).map_err(|e| e.to_string())?;
    let generation = session
        .continue_greedy(MAX_NEW)
        .map_err(|e| e.to_string())?;
    Ok(generation.token_ids)
}

fn main() {
    // Reference: big model alone in the process.
    let reference = {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_big_once());
        });
        rx.recv().unwrap().expect("solo big-model run failed")
    };
    println!("solo 9B tokens: {:?}", reference);

    // Interference run: small model generating in a loop while the big model
    // runs the same prompt again.
    let stop = Arc::new(AtomicBool::new(false));
    let small_started = Arc::new(AtomicBool::new(false));
    let small_thread = {
        let stop = stop.clone();
        let small_started = small_started.clone();
        std::thread::spawn(move || -> Result<usize, String> {
            let model = LlamaModel::load(std::path::Path::new(SMALL_MODEL)).map_err(|e| e.to_string())?;
            let vocab = LlamaVocab::from_model(&model).map_err(|e| e.to_string())?;
            let mut rounds = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let mut session = LlamaSession::from_model(
                    &model,
                    LlamaSessionConfig {
                        max_context: Some(2048),
                        ..LlamaSessionConfig::default()
                    },
                )
                .map_err(|e| e.to_string())?;
                let tokens = vocab
                    .tokenize(SMALL_PROMPT, true, true)
                    .map_err(|e| e.to_string())?;
                session.append_tokens(&tokens).map_err(|e| e.to_string())?;
                let _ = session
                    .continue_greedy(16)
                    .map_err(|e| e.to_string())?;
                small_started.store(true, Ordering::Relaxed);
                rounds += 1;
            }
            Ok(rounds)
        })
    };

    // Wait until the small model is actively generating before starting.
    while !small_started.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let concurrent = {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_big_once());
        });
        rx.recv().unwrap().expect("concurrent big-model run failed")
    };
    stop.store(true, Ordering::Relaxed);
    let rounds = small_thread.join().unwrap().expect("small-model loop failed");
    println!("concurrent 9B tokens: {:?}", concurrent);
    println!("small-model rounds completed: {}", rounds);

    if concurrent == reference {
        println!("RESULT: IDENTICAL (no cross-session interference)");
    } else {
        println!("RESULT: MISMATCH — cross-session interference detected");
        std::process::exit(1);
    }
}
