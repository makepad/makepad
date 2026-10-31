//! Train it yourself: build music training data from your song libraries
//! and your own files, and score a model's answers.
//!
//!     music_corpus build --sources lmd[,folder:<dir>...] --out <dir> [--context 8192]
//!         [--lmd <dir>] [--tokenizer <qwen.gguf>] [--threads 8] [--min-quality 70]
//!         [--describe 2] [--continue 2] [--infill 2] [--score 1] [--augment 1]
//!         [--val 50] [--test 50] (per mille) [--max-songs N]
//!     music_corpus stats <dir>
//!     music_corpus baseline --data <dir> --out <pred.jsonl> [--split test] [--kind heuristic|oracle] [--limit 200]
//!     music_corpus eval --data <dir> --pred <pred.jsonl> [--split test] [--threads 8]
//!         [--render N --wav <dir>] [--json <report.json>]
//!
//! `lmd` is the Lakh MIDI clean subset as `midi_library install` and
//! `midi_library index` left it; `folder:<dir>` reads .mid, .mod, .xm,
//! .s3m, .it and .sid files under a folder. The output stays yours: never
//! redistribute it.

use makepad_music_corpus::corpus::{self, Opts};
use makepad_music_corpus::eval;
use makepad_music_corpus::source::{Folder, Lakh, Source};
use makepad_music_corpus::tokens::Tokenizer;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let num = |name: &str, default: usize| flag(name).and_then(|v| v.parse().ok()).unwrap_or(default);
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&flag, &num),
        Some("stats") => match args.get(1) {
            Some(dir) => corpus::print_stats(Path::new(dir)).map(|t| print!("{t}")),
            None => Err("music_corpus stats <dir>".into()),
        },
        Some("baseline") => baseline(&flag, &num),
        Some("eval") => run_eval(&flag, &num),
        _ => Err("music_corpus build | stats | baseline | eval (see the source's header for the options)".into()),
    };
    if let Err(e) = result {
        eprintln!("music_corpus: {e}");
        std::process::exit(1);
    }
}

type Flag<'a> = dyn Fn(&str) -> Option<String> + 'a;
type Num<'a> = dyn Fn(&str, usize) -> usize + 'a;

fn build(flag: &Flag, num: &Num) -> Result<(), String> {
    let out = PathBuf::from(flag("--out").ok_or("--out <dir>")?);
    let mut opts = Opts { out, ..Opts::default() };
    opts.limits.context = num("--context", opts.limits.context);
    opts.threads = num("--threads", opts.threads);
    opts.min_quality = num("--min-quality", opts.min_quality as usize) as u8;
    opts.limits.describe = num("--describe", opts.limits.describe);
    opts.limits.cont = num("--continue", opts.limits.cont);
    opts.limits.infill = num("--infill", opts.limits.infill);
    opts.limits.score = num("--score", opts.limits.score);
    opts.limits.augment = num("--augment", opts.limits.augment);
    opts.val = num("--val", opts.val as usize) as u32;
    opts.test = num("--test", opts.test as usize) as u32;
    opts.max_songs = flag("--max-songs").and_then(|v| v.parse().ok());
    let tok_path = flag("--tokenizer").map(PathBuf::from).or_else(Tokenizer::find).ok_or("no Qwen GGUF found for the tokenizer (--tokenizer <file.gguf>)")?;
    let tok = Tokenizer::open(&tok_path)?;
    eprintln!("tokenizer: {} ({} pieces)", tok_path.display(), tok.pieces());
    let mut sources: Vec<Box<dyn Source>> = Vec::new();
    for s in flag("--sources").unwrap_or_else(|| "lmd".into()).split(',').filter(|s| !s.is_empty()) {
        if s == "lmd" {
            eprintln!("reading the Lakh MIDI archive");
            sources.push(Box::new(Lakh::open(flag("--lmd").as_deref().map(Path::new))?));
        } else if let Some(dir) = s.strip_prefix("folder:") {
            sources.push(Box::new(Folder::open(Path::new(dir))?));
        } else {
            return Err(format!("unknown source `{s}` (lmd, folder:<dir>)"));
        }
    }
    let sum = corpus::build(&sources, &tok, &opts, &|m| eprintln!("{m}"))?;
    eprintln!("built in {:.0} s", sum.seconds);
    print!("{}", corpus::print_stats(&opts.out)?);
    Ok(())
}

fn baseline(flag: &Flag, num: &Num) -> Result<(), String> {
    let data = PathBuf::from(flag("--data").ok_or("--data <dir>")?);
    let out = flag("--out").ok_or("--out <pred.jsonl>")?;
    let cases = eval::load_cases(&data, &flag("--split").unwrap_or_else(|| "test".into()))?;
    let kind = flag("--kind").unwrap_or_else(|| "heuristic".into());
    std::fs::write(&out, eval::baseline(&cases, &kind, num("--limit", 200))).map_err(|e| format!("{out}: {e}"))?;
    eprintln!("wrote {kind} answers to {out}");
    Ok(())
}

fn run_eval(flag: &Flag, num: &Num) -> Result<(), String> {
    let data = PathBuf::from(flag("--data").ok_or("--data <dir>")?);
    let pred_path = flag("--pred").ok_or("--pred <pred.jsonl>")?;
    let cases = eval::load_cases(&data, &flag("--split").unwrap_or_else(|| "test".into()))?;
    let pred = std::fs::read_to_string(&pred_path).map_err(|e| format!("{pred_path}: {e}"))?;
    let wav = flag("--wav").map(PathBuf::from);
    let render = flag("--render").and_then(|n| n.parse().ok()).zip(wav.as_deref());
    let rep = eval::evaluate(&cases, &pred, num("--threads", 8), render)?;
    print!("{}", eval::report_text(&rep));
    if let Some(path) = flag("--json") {
        std::fs::write(&path, eval::report_json(&rep).to_json()).map_err(|e| format!("{path}: {e}"))?;
    }
    Ok(())
}
