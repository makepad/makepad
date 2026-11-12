// Calls into the C crate it build-depends on, with that crate's build-script cfg, and counts its
// own runs in OUT_DIR: what the crate includes says what the script saw and how often it ran.
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let runs_path = format!("{}/runs", out);
    let runs = std::fs::read_to_string(&runs_path).ok().and_then(|s| s.trim().parse::<u32>().ok()).unwrap_or(0) + 1;
    std::fs::write(&runs_path, runs.to_string()).unwrap();
    std::fs::write(
        format!("{}/generated.rs", out),
        format!("pub const ANSWER: i32 = {};\npub const BUILT: bool = {};\npub const RUNS: u32 = {};\n", cnative::answer(), cnative::built(), runs),
    ).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
