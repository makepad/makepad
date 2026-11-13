// Compiles csrc/callback.c with the system C compiler ($CC or cc) into a static library in OUT_DIR, as
// the cc crate would, and links it.
use std::process::Command;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let obj = format!("{out}/callback.o");
    assert!(Command::new(&cc).args(["-O2", "-fPIC", "-c", "csrc/callback.c", "-o", &obj]).status().expect("cc").success(), "cc failed");
    let lib = format!("{out}/libsnd_callback.a");
    let _ = std::fs::remove_file(&lib);
    assert!(Command::new("ar").args(["crs", &lib, &obj]).status().expect("ar").success(), "ar failed");
    println!("cargo:rerun-if-changed=csrc/callback.c");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=snd_callback");
}
