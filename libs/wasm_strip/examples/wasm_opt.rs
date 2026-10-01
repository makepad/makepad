//! Runs the optimiser on a wasm file:
//! `wasm_opt <in.wasm> [out.wasm] [--keep-names] [--passes strip,dce,...]`.
use makepad_wasm_strip::*;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let input = std::fs::read(&args[1]).expect("read input");
    let mut opts = OptimizeOptions {
        keep_names: args.iter().any(|arg| arg == "--keep-names"),
        ..OptimizeOptions::default()
    };
    if let Some(at) = args.iter().position(|arg| arg == "--passes") {
        let passes: Vec<&str> = args[at + 1].split(',').collect();
        let on = |name: &str| passes.contains(&name);
        opts.strip = on("strip");
        opts.dce = on("dce");
        opts.peephole = on("peephole");
        opts.locals = on("locals");
        opts.merge = on("merge");
        opts.compact = on("compact");
        opts.order = on("order");
    }
    let start = std::time::Instant::now();
    match wasm_optimize_checked(&input, &opts) {
        Ok((output, report)) => {
            print!("{}", report.to_text());
            println!("took {:?}", start.elapsed());
            if let Some(path) = args.get(2).filter(|arg| !arg.starts_with("--")) {
                std::fs::write(path, output).expect("write output");
            }
        }
        Err(msg) => {
            eprintln!("error: {msg}");
            std::process::exit(1);
        }
    }
}
