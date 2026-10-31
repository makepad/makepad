//! The module-wise strip by hand:
//! - `wasm_units instrument <in.wasm> <out.wasm> [--phase-mark <name part>]...`
//! - `wasm_units coverage <instrumented.wasm> <coverage memory dump> <out.txt>`
//! - `wasm_units strip <in.wasm> <coverage.txt> <out.wasm> [--keep <unit>]... [--keep-names]`
//! - `wasm_units modules <in.wasm> <collect manifest dir> <out.wasm> [--keep <module>]... [--keep-names] [--prune-exported-table]`:
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
                prune_exported_table: args.iter().any(|a| a == "--prune-exported-table"),
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
                prune_exported_table: args.iter().any(|a| a == "--prune-exported-table"),
                ..OptimizeOptions::default()
            };
            let (out, report) = wasm_optimize_checked(&read(&args[2]), &opts).unwrap_or_else(|e| panic!("{e}"));
            print!("{}", report.units.clone().unwrap_or_default());
            print!("{}", report.to_text());
            std::fs::write(&args[4], out).expect("write");
        }
        Some("report") => {
            // Per unit of a named module: code bytes, functions, and how many
            // a coverage run (of a build with the same code, matched by
            // demangled name) entered.
            use makepad_wasm_strip::opt::demangle::demangle;
            let module = makepad_wasm_strip::opt::ir::decode(&read(&args[2])).expect("decode");
            let coverage = Coverage::parse(&String::from_utf8(read(&args[3])).expect("utf8"));
            let readable = |s: &str| demangle(s).map(|d| d.name).unwrap_or_else(|| s.to_string());
            let entered: std::collections::HashMap<String, u8> = coverage.entered.iter().map(|(s, p)| (readable(s), *p)).collect();
            let imported = module.num_imported_funcs();
            let mut units: std::collections::BTreeMap<String, (usize, usize, usize)> = Default::default();
            for (i, sym) in &module.names.as_ref().expect("names").funcs {
                if *i < imported {
                    continue;
                }
                let Some(unit) = makepad_wasm_strip::opt::units::unit_of(sym) else { continue };
                let mut body = Vec::new();
                makepad_wasm_strip::opt::encode::func_body(&mut body, &module.funcs[(*i - imported) as usize]);
                let e = units.entry(unit).or_default();
                e.0 += body.len();
                e.1 += 1;
                e.2 += usize::from(entered.contains_key(&readable(sym)));
            }
            let mut rows: Vec<_> = units.into_iter().collect();
            rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
            let (mut never, mut total) = (0, 0);
            for (_, (bytes, _, hit)) in &rows {
                total += bytes;
                if *hit == 0 {
                    never += bytes;
                }
            }
            println!("code {total} bytes; in units the run never entered: {never}");
            for (unit, (bytes, fns, hit)) in rows.iter().filter(|r| std::env::var("NEVER").is_err() || r.1 .2 == 0).take(60) {
                println!("{bytes:>9} {fns:>5} fns {hit:>5} entered  {unit}");
            }
        }
        _ => eprintln!("wasm_units instrument|coverage|strip ... (see the source)"),
    }
}
