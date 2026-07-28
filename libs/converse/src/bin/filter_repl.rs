//! Interactive harness for the transcript filter: type utterances, see the
//! SEND/SKIP verdict and latency without any app around it.
//!
//! Usage: filter-repl [model.gguf]
//!   (default model: local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf)
//!
//! Lines are judged as heard utterances. Prefixes:
//!   u: <text>   add a user line to the dialog context without judging
//!   a: <text>   add an assistant line to the dialog context without judging
//! A forwarded utterance is added to the context automatically.

use makepad_converse::filter::{FilterDecision, TranscriptFilter};
use makepad_converse::qwen_filter::QwenFilter;
use std::io::{BufRead, Write};

const DEFAULT_MODEL: &str = "local/models/Qwen3.5-9B-UD-Q4_K_XL.gguf";

fn main() {
    let model_path = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let mut filter = QwenFilter::new(
        &model_path,
        "The assistant builds and edits a 3D game world while the user watches it change.",
    );
    let mut dialog: Vec<String> = Vec::new();

    eprintln!("filter-repl: {model_path} (loads on first judgement)");
    let stdin = std::io::stdin();
    loop {
        eprint!("> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("u:") {
            dialog.push(format!("user: {}", rest.trim()));
            continue;
        }
        if let Some(rest) = line.strip_prefix("a:") {
            dialog.push(format!("assistant: {}", rest.trim()));
            continue;
        }
        let started = std::time::Instant::now();
        let decision = filter.judge(line, &dialog);
        let secs = started.elapsed().as_secs_f64();
        match decision {
            FilterDecision::Forward { instruction } => {
                println!("SEND ({secs:.2}s): {instruction}");
                dialog.push(format!("user: {instruction}"));
            }
            FilterDecision::Drop { reason } => {
                println!("skip ({secs:.2}s): {reason}");
            }
        }
    }
}
