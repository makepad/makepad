//! Offline queries on a saved recording: `cargo run --release -p makepad-gpu-trace --example query --
//! <file.mkgp> frames|frame <F>|lanes <F>|top by=..|util|diff <a> <b> [as=md]` (the same answers as
//! `makepad inspect file=<x.mkgp> gpu prof ...`, without building the loader).

use makepad_gpu_trace::query::{self, Args};
use makepad_gpu_trace::Recording;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("query <file.mkgp> <query words...>");
        std::process::exit(2);
    };
    let load = |p: &str| -> Result<Recording, String> { Recording::decode(&std::fs::read(p).map_err(|e| format!("cannot read {p}: {e}"))?) };
    let rec = match load(path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let words = &args[1..];
    match query::run(&rec, &Args::parse(words), &load) {
        Ok(a) if words.iter().any(|w| w == "as=md") => query::markdown(&a).iter().for_each(|l| println!("{l}")),
        Ok(a) => {
            a.lines.iter().for_each(|l| println!("{l}"));
            a.findings.iter().for_each(|f| println!("- {f}"));
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
