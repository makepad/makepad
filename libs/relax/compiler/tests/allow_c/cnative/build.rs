// Compiles csrc/cnative.c with the system C compiler ($CC or cc) into a static library in OUT_DIR,
// as the cc crate would, and links it.
use std::process::Command;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let obj = format!("{}/cnative.o", out);
    let ok = Command::new(&cc).args(["-O2", "-fPIC", "-c", "csrc/cnative.c", "-o", &obj]).status().expect("cc").success();
    assert!(ok, "cc failed");
    let lib = format!("{}/libcnative.a", out);
    let _ = std::fs::remove_file(&lib);
    let ok = Command::new("ar").args(["crs", &lib, &obj]).status().expect("ar").success();
    assert!(ok, "ar failed");
    println!("cargo:rerun-if-changed=csrc/cnative.c");
    println!("cargo:rustc-link-search=native={}", out);
    println!("cargo:rustc-link-lib=static=cnative");
    println!("cargo:rustc-cfg=cnative_built");
}
