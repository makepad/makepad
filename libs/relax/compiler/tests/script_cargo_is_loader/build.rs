// A build script's CARGO is the loader, as its RUSTC is: `$CARGO metadata` is answered from the
// run's own manifests (a virtual or shared workspace's are not on the disk a cargo reads), also
// when the run was started with a cargo of its own (CARGO, as `cargo run` sets it).
fn main() {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let manifest = format!("{}/Cargo.toml", var("CARGO_MANIFEST_DIR"));
    let out = std::process::Command::new(var("CARGO")).args(["metadata", "--format-version", "1", "--no-deps", "--manifest-path", &manifest]).output();
    let metadata = match out {
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("script-cargo-app") => "ok".to_string(),
        Ok(o) => format!("failed: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("")),
        Err(e) => format!("not run: {e}"),
    };
    let tool = if var("CARGO") == var("RUSTC") { "the loader" } else { "not the loader" };
    println!("cargo:rustc-env=SCRIPT_CARGO=CARGO is {tool}, metadata {metadata}");
}
