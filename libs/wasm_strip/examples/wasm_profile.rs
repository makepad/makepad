//! Prints a size profile: `wasm_profile <file.wasm> [--depth N] [--top N] [--json]`,
//! or a per-crate diff: `wasm_profile <before.wasm> --diff <after.wasm>`.
use makepad_wasm_strip::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut opts = ProfileOptions::default();
    let mut top = 30;
    let mut json = false;
    let mut diff = None;
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--depth" => {
                i += 1;
                opts.module_depth = args[i].parse().expect("depth");
            }
            "--top" => {
                i += 1;
                top = args[i].parse().expect("top");
            }
            "--json" => json = true,
            "--diff" => {
                i += 1;
                diff = Some(args[i].clone());
            }
            other => panic!("unknown argument {other}"),
        }
        i += 1;
    }
    let read = |path: &str| {
        let buf = std::fs::read(path).expect("read wasm");
        wasm_size_profile(&buf, &opts).expect("parse wasm")
    };
    let profile = read(&args[1]);
    if let Some(after) = diff {
        print!("{}", profile.diff_text(&read(&after), top));
    } else if json {
        println!("{}", profile.to_json(top));
    } else {
        print!("{}", profile.to_text(top));
    }
}
