//! h3-tokenize: CLI for the MiniMax H3 Qwen2 byte-level BPE tokenizer.
//! Parity target: transformers AutoTokenizer, add_special_tokens=False.
//!
//! Usage:
//!   h3-tokenize --dir <tokenizer dir> --prompt "text"
//!       prints token count + ids
//!   h3-tokenize --dir <tokenizer dir> --fixture <tok_ref.json>
//!       runs all fixture prompts, PASS/FAIL per prompt, prints TOKPARITY:OK
//!       and exits 0 only when every prompt matches exactly

use makepad_diffusion::h3_tokenizer::{H3Tokenizer, Json};
use std::collections::HashMap;
use std::path::Path;

const USAGE: &str =
    "usage: h3-tokenize --dir <tokenizer dir> (--prompt \"text\" | --fixture <tok_ref.json>)";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts: HashMap<String, String> = HashMap::new();
    let mut key: Option<String> = None;
    for arg in &args[1..] {
        if let Some(name) = arg.strip_prefix("--") {
            key = Some(name.to_string());
            opts.entry(name.to_string()).or_default();
        } else if let Some(name) = key.take() {
            opts.insert(name, arg.clone());
        }
    }
    if let Err(err) = run(&opts) {
        eprintln!("h3-tokenize FAILED: {err}");
        std::process::exit(1);
    }
}

fn run(opts: &HashMap<String, String>) -> Result<(), String> {
    let dir = opts
        .get("dir")
        .filter(|value| !value.is_empty())
        .ok_or(USAGE)?;
    let tokenizer = H3Tokenizer::load(Path::new(dir)).map_err(|err| err.to_string())?;

    if let Some(prompt) = opts.get("prompt") {
        let ids = tokenizer.encode(prompt);
        println!("{} tokens", ids.len());
        println!(
            "{}",
            ids.iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        return Ok(());
    }
    if let Some(fixture) = opts.get("fixture").filter(|value| !value.is_empty()) {
        return run_fixture(&tokenizer, Path::new(fixture));
    }
    Err(USAGE.to_string())
}

fn run_fixture(tokenizer: &H3Tokenizer, path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let root = Json::parse(&text).map_err(|msg| format!("{}: {msg}", path.display()))?;
    let prompts = root
        .get("prompts")
        .and_then(Json::as_arr)
        .ok_or_else(|| format!("{}: no \"prompts\" array", path.display()))?;
    if prompts.is_empty() {
        return Err(format!("{}: empty \"prompts\" array", path.display()));
    }

    let mut failed = 0;
    for prompt in prompts {
        let name = prompt.get("name").and_then(Json::as_str).unwrap_or("?");
        let Some(prompt_text) = prompt.get("text").and_then(Json::as_str) else {
            println!("FAIL {name}: fixture entry has no \"text\" string");
            failed += 1;
            continue;
        };
        let Some(expected) = prompt.get("ids").and_then(Json::as_arr).map(|ids| {
            ids.iter()
                .filter_map(Json::as_u32)
                .collect::<Vec<u32>>()
        }) else {
            println!("FAIL {name}: fixture entry has no \"ids\" array");
            failed += 1;
            continue;
        };

        let got = tokenizer.encode(prompt_text);
        if got == expected {
            println!("PASS {name} ({} tokens)", got.len());
            continue;
        }
        failed += 1;
        let diff = got
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| got.len().min(expected.len()));
        println!(
            "FAIL {name}: expected {} tokens, got {}; first diff at index {diff} \
             (expected {:?}, got {:?})",
            expected.len(),
            got.len(),
            expected.get(diff),
            got.get(diff),
        );
        let window = |ids: &[u32]| -> Vec<u32> {
            ids[diff.saturating_sub(3)..(diff + 4).min(ids.len())].to_vec()
        };
        println!("  expected[..around diff]: {:?}", window(&expected));
        println!("  got[..around diff]:      {:?}", window(&got));
    }

    if failed > 0 {
        return Err(format!("{failed} fixture prompt(s) mismatched"));
    }
    println!("TOKPARITY:OK");
    Ok(())
}
