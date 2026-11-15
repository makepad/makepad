// Relax's grants (options.rs GRANTS) come from the person's command line alone: cargo's `[env]`
// and a build script's `rustc-env` name them (.cargo/config.toml, build.rs), a test puts them in
// the environment, and the program names two for its self-exec child (one set in its own
// environment, one in the spawn's). Prints what the build script saw and did, then for this run
// and its child whether a module's `unsafe` loads, whether fuel stops a long module call, and what
// the program reads of two of the names.
const UNSAFE: &str = "pub fn frame(x: u64) -> u64 { let p = &x as *const u64; unsafe { *p } }";
const SPIN: &str = "pub fn spin(n: u64) -> u64 { let mut x = 1u64; let mut i = 0; while i < n { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); i += 1; } x }";

fn checks(who: &str) {
    match std::relax::Module::load_source("esc", "esc.rs", UNSAFE, &[]) {
        Err(e) if e.contains("not allowed in a sandboxed module") => println!("{who}: unsafe module refused"),
        Err(e) => println!("{who}: unsafe module failed otherwise: {e}"),
        Ok(_) => println!("{who}: unsafe module LOADED"),
    }
    let m = std::relax::Module::load_source("spin", "spin.rs", SPIN, &[]).unwrap();
    let spin: fn(u64) -> u64 = m.get("spin").unwrap();
    println!("{who}: long call stopped: {}", std::relax::guard(0, || spin(100_000_000)).is_err());
    let read = |n: &str| std::env::var(n).unwrap_or_else(|_| "nothing".into());
    println!("{who}: program reads RELAX_SANDBOX_FUEL={} RELAX_MODULE_FUEL={}", read("RELAX_SANDBOX_FUEL"), read("RELAX_MODULE_FUEL"));
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("child") {
        checks("child");
        return;
    }
    println!("script: {}", include_str!(concat!(env!("OUT_DIR"), "/seen.txt")));
    checks("run");
    // the program's own values: its child program reads them, the child's host takes none
    std::env::set_var("RELAX_MODULE_FUEL", "1");
    let out = std::process::Command::new(std::env::current_exe().unwrap()).arg("child").env("RELAX_MODULES_TRUSTED", "1").output().unwrap();
    print!("{}", String::from_utf8_lossy(&out.stdout));
    if !out.status.success() {
        println!("child failed: {}", String::from_utf8_lossy(&out.stderr));
    }
}
