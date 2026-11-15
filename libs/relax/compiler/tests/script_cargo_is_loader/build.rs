// A build script's CARGO is the loader, as its RUSTC is: `$CARGO metadata` is answered from the
// run's own manifests (a virtual or shared workspace's are not on the disk a cargo reads), also
// when the run was started with a cargo of its own (CARGO, as `cargo run` sets it). `$RUSTC`'s
// questions are answered by the script's own process: no loader starts for them, and a child
// answered there has the script's process id.
use std::io::Write;
use std::process::{Command, Stdio};

fn main() {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let manifest = format!("{}/Cargo.toml", var("CARGO_MANIFEST_DIR"));
    let out = Command::new(var("CARGO")).args(["metadata", "--format-version", "1", "--no-deps", "--manifest-path", &manifest]).output();
    let metadata = match out {
        Ok(o) if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("script-cargo-app") => "ok".to_string(),
        Ok(o) => format!("failed: {}", String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("")),
        Err(e) => format!("not run: {e}"),
    };
    let tool = if var("CARGO") == var("RUSTC") { "the loader" } else { "not the loader" };
    println!("cargo:rustc-env=SCRIPT_CARGO=CARGO is {tool}, metadata {metadata}");

    // (whether this process answered, stdout, exit code)
    let ask = |args: &[&str], input: Option<&str>| -> (bool, String, i32) {
        let mut c = Command::new(var("RUSTC"));
        c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
        if input.is_some() {
            c.stdin(Stdio::piped());
        }
        let mut child = c.spawn().expect("RUSTC starts");
        let here = child.id() == std::process::id();
        if let Some(i) = input {
            child.stdin.take().unwrap().write_all(i.as_bytes()).unwrap();
        }
        let o = child.wait_with_output().unwrap();
        (here, String::from_utf8_lossy(&o.stdout).into_owned(), o.status.code().unwrap_or(-1))
    };
    let (v_here, version, _) = ask(&["--version"], None);
    let (c_here, cfg, _) = ask(&["--print", "cfg"], None);
    // a compile probe (autocfg's shape: the program on stdin): Relax is not rustc, the probe fails
    let out_dir = var("OUT_DIR");
    let (p_here, _, probe) = ask(&["--crate-name", "probe", "--crate-type=lib", "--emit=metadata", "--out-dir", &out_dir, "-"], Some("pub fn probe() {}"));
    // Command::output, the common form
    let vv = Command::new(var("RUSTC")).arg("-vV").output().map(|o| String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").to_string()).unwrap_or_default();
    let here = if v_here && c_here && p_here { "here" } else { "by another process" };
    println!("cargo:rustc-env=SCRIPT_RUSTC=RUSTC answered {here}, compile probe status {probe}, -vV {}", if vv == version.trim() { "agrees" } else { "differs" });
    println!("cargo:rustc-env=SCRIPT_RUSTC_VERSION={}", version.trim());
    println!("cargo:rustc-env=SCRIPT_RUSTC_CFG={}", cfg.lines().collect::<Vec<_>>().join(","));
}
