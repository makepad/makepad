// Compiles csrc/value.c with the C compiler ($CC, the fixture's) into a static library in OUT_DIR,
// as the cc crate would, and links it.
use std::process::Command;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let obj = format!("{out}/value.o");
    assert!(Command::new(&cc).args(["-O2", "-fPIC", "-c", "csrc/value.c", "-o", &obj]).status().expect("cc").success(), "cc failed");
    let lib = format!("{out}/libsn_native.a");
    let _ = std::fs::remove_file(&lib);
    assert!(Command::new("ar").args(["crs", &lib, &obj]).status().expect("ar").success(), "ar failed");
    println!("cargo:rerun-if-changed=csrc/value.c");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=sn_native");
}
