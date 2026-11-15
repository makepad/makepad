// Links libdemo, a shared library in DEMO_SDK_DIR (a folder outside the dynamic loader's search
// path, where an SDK is installed: verify.py installs it before any build), and builds a static
// shim in OUT_DIR that calls it, with the system C compiler ($CC or cc) and ar. It writes only its
// OUT_DIR, as cargo asks of a build script: the optimized native build runs it again beside a
// start, and a start loaded a libdemo it was writing there ("file too short").
use std::process::Command;

fn run(cmd: &mut Command) {
    assert!(cmd.status().expect("start the tool").success(), "{cmd:?} failed");
}

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let sdk = std::env::var("DEMO_SDK_DIR").expect("DEMO_SDK_DIR: the folder libdemo is installed in");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    run(Command::new(&cc).args(["-O2", "-fPIC", "-c", "csrc/shim.c", "-o", &format!("{out}/shim.o")]));
    let lib = format!("{out}/libshim.a");
    let _ = std::fs::remove_file(&lib);
    run(Command::new("ar").args(["crs", &lib, &format!("{out}/shim.o")]));
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rerun-if-env-changed=DEMO_SDK_DIR");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=shim");
    println!("cargo:rustc-link-search=native={sdk}");
    println!("cargo:rustc-link-lib=dylib=demo");
}
