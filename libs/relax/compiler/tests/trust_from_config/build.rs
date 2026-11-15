// A path package's build script: what it sees of Relax's grants, what its host does with a
// module's `unsafe` (the script's host gets `--allow-c` alone of the run's grants), and
// `rustc-env` lines that name two grants (variables for the program, never for Relax).
const UNSAFE: &str = "pub fn frame(x: u64) -> u64 { let p = &x as *const u64; unsafe { *p } }";

fn main() {
    let names = ["RELAX_MODULES_TRUSTED", "RELAX_SANDBOX_NOCHECK", "RELAX_ALLOW_C", "RELAX_SANDBOX_FUEL", "RELAX_MODULE_FUEL"];
    let seen: Vec<String> = names.iter().filter_map(|n| std::env::var(n).ok().map(|v| format!("{n}={v}"))).collect();
    let module = match std::relax::Module::load_source("esc", "esc.rs", UNSAFE, &[]) {
        Err(e) if e.contains("not allowed in a sandboxed module") => "refused".to_string(),
        Err(e) => format!("failed otherwise: {e}"),
        Ok(_) => "LOADED".to_string(),
    };
    let out = std::env::var("OUT_DIR").unwrap();
    std::fs::write(format!("{out}/seen.txt"), format!("saw {}, its unsafe module {module}", if seen.is_empty() { "none".to_string() } else { seen.join(" ") })).unwrap();
    println!("cargo:rustc-env=RELAX_SANDBOX_NOCHECK=1");
    println!("cargo:rustc-env=RELAX_SANDBOX_FUEL=0");
}
