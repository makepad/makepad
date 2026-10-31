//! Runs every `*.txt` chat prompt of a directory (system message, user
//! message, assistant header) as a TurnEngine turn and prints the replies
//! with their timings: the first system prompt is prepared before any
//! turn, and a repeated system prompt is prefilled once.
//!
//! A prompt `NAME.txt` with a `NAME.ppm` next to it is an image turn (needs
//! the mmproj argument).
//!
//! usage: llama-turns <model.gguf> <prompt-dir> [max-new-tokens] [mmproj.gguf] [--spec-draft-max N]

use makepad_ai_llm::turns::{TurnConfig, TurnEngine, TurnEvent, TurnImage, TurnRequest};
use std::time::Duration;

fn split(prompt: &str) -> Option<(String, String)> {
    let sys = prompt.split("<|im_start|>system\n").nth(1)?.split("<|im_end|>").next()?.to_string();
    let user = prompt.split("<|im_start|>user\n").nth(1)?.split("<|im_end|>").next()?.to_string();
    Some((sys, user))
}

fn main() {
    let mut args = Vec::new();
    let mut spec_draft_max = 0usize;
    let mut cli = std::env::args();
    while let Some(arg) = cli.next() {
        if arg == "--spec-draft-max" {
            spec_draft_max = cli.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| {
                eprintln!("--spec-draft-max requires a nonnegative integer");
                std::process::exit(2);
            });
        } else {
            args.push(arg);
        }
    }
    if args.len() < 3 {
        eprintln!("usage: llama-turns <model.gguf> <prompt-dir> [max-new-tokens] [mmproj.gguf] [--spec-draft-max N]");
        std::process::exit(2);
    }
    let max_new = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(80);
    let mut files: Vec<_> = std::fs::read_dir(&args[2]).expect("prompt dir").flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "txt")).collect();
    files.sort();
    let mut config = TurnConfig::new(args[1].clone().into());
    config.spec_draft_max = spec_draft_max;
    config.mmproj = args.get(4).map(Into::into);
    // Image turns hold ~256 image tokens plus the text.
    if config.mmproj.is_some() {
        config.max_context = 4096;
    }
    let engine = TurnEngine::start(config, None);
    let wait = |engine: &TurnEngine| loop {
        let events = engine.poll();
        if !events.is_empty() {
            return events;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    'ready: loop {
        for e in wait(&engine) {
            match e {
                TurnEvent::Ready { device, load_ms } => {
                    println!("ready on {device} in {load_ms:.0} ms");
                    break 'ready;
                }
                TurnEvent::LoadFailed { error } => panic!("load failed: {error}"),
                _ => {}
            }
        }
    }
    // Prefill the first prompt's system message before any turn, as an app
    // does while the user is still speaking.
    if let Some((system, _)) = files.first().and_then(|f| split(&std::fs::read_to_string(f).ok()?)) {
        engine.prepare(system);
        'prepared: loop {
            for e in wait(&engine) {
                match e {
                    TurnEvent::Prepared { prefill_ms } => {
                        println!("system prompt prepared in {prefill_ms:.0} ms");
                        break 'prepared;
                    }
                    TurnEvent::PrepareFailed { error } => panic!("prepare failed: {error}"),
                    _ => {}
                }
            }
        }
    }
    for (i, file) in files.iter().enumerate() {
        let prompt = std::fs::read_to_string(file).expect("prompt");
        let Some((system, user)) = split(&prompt) else { continue };
        let id = i as u64 + 1;
        let image = makepad_ai_llm::read_ppm(&file.with_extension("ppm").to_string_lossy()).ok()
            .map(|(rgb, width, height)| TurnImage { rgb, width, height });
        engine.submit(TurnRequest { id, system, user, image, max_new_tokens: max_new });
        'turn: loop {
            for e in wait(&engine) {
                match e {
                    TurnEvent::Done { text, stats, .. } => {
                        println!(
                            "{} | vision {:.0} ms | prefill {} tok {:.0} ms (reused {}) | {} tok {:.0} ms ({:.1} tok/s) | {}",
                            file.file_name().unwrap().to_string_lossy(),
                            stats.vision_ms,
                            stats.prefill_tokens,
                            stats.prefill_ms,
                            stats.prefix_reused,
                            stats.tokens,
                            stats.decode_ms,
                            stats.tokens as f64 / (stats.decode_ms / 1e3).max(1e-9),
                            text.trim()
                        );
                        break 'turn;
                    }
                    TurnEvent::Failed { error, .. } => {
                        println!("{}: FAILED {error}", file.display());
                        break 'turn;
                    }
                    _ => {}
                }
            }
        }
    }
}
