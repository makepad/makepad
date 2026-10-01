//! The module-wise strip by hand:
//! - `wasm_units instrument <in.wasm> <out.wasm> [--phase-mark <name part>]...`
//! - `wasm_units coverage <instrumented.wasm> <coverage memory dump> <out.txt>`
//! - `wasm_units strip <in.wasm> <coverage.txt> <out.wasm> [--keep <unit>]... [--keep-names]`
//! - `wasm_units modules <in.wasm> <collect manifest dir> <out.wasm> [--keep <module>]... [--keep-names]`:
//!   the registration strip from the manifest's `modules.txt` and
//!   `modules-registered.txt`
use makepad_wasm_strip::*;

fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    args.windows(2).filter(|w| w[0] == flag).map(|w| w[1].clone()).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let read = |p: &str| std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}"));
    match args.get(1).map(|s| s.as_str()) {
        Some("instrument") => {
            let out = wasm_instrument_coverage(&read(&args[2]), &flag_values(&args, "--phase-mark")).unwrap_or_else(|e| panic!("{e}"));
            std::fs::write(&args[3], out).expect("write");
        }
        Some("coverage") => {
            let cov = wasm_coverage(&read(&args[2]), &read(&args[3])).unwrap_or_else(|e| panic!("{e}"));
            let used = cov.entered.values().filter(|p| **p >= 2).count();
            println!("{} functions entered, {} after registration", cov.entered.len(), used);
            std::fs::write(&args[4], cov.to_text()).expect("write");
        }
        Some("strip") => {
            let coverage = Coverage::parse(&String::from_utf8(read(&args[3])).expect("utf8"));
            let opts = OptimizeOptions {
                coverage: Some(coverage),
                keep_units: flag_values(&args, "--keep"),
                keep_names: args.iter().any(|a| a == "--keep-names"),
                ..OptimizeOptions::default()
            };
            let start = std::time::Instant::now();
            let (out, report) = wasm_optimize_checked(&read(&args[2]), &opts).unwrap_or_else(|e| panic!("{e}"));
            print!("{}", report.units.clone().unwrap_or_default());
            print!("{}", report.to_text());
            println!("took {:?}", start.elapsed());
            std::fs::write(&args[4], out).expect("write");
        }
        Some("modules") => {
            let dir = std::path::Path::new(&args[3]);
            let lines = |f: &str| -> std::collections::BTreeSet<String> {
                std::fs::read_to_string(dir.join(f)).unwrap_or_else(|e| panic!("{f}: {e}")).lines().filter_map(|l| l.split('\t').next()).filter(|l| !l.is_empty()).map(String::from).collect()
            };
            let modules = ModuleUse { registered: lines("modules-registered.txt"), used: lines("modules.txt") };
            let opts = OptimizeOptions {
                modules: Some(modules),
                keep_units: flag_values(&args, "--keep"),
                keep_names: args.iter().any(|a| a == "--keep-names"),
                ..OptimizeOptions::default()
            };
            let (out, report) = wasm_optimize_checked(&read(&args[2]), &opts).unwrap_or_else(|e| panic!("{e}"));
            print!("{}", report.units.clone().unwrap_or_default());
            print!("{}", report.to_text());
            std::fs::write(&args[4], out).expect("write");
        }
        _ => eprintln!("wasm_units instrument|coverage|strip ... (see the source)"),
    }
}
