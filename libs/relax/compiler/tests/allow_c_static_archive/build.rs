// Builds a static archive with the system tools it runs itself (no `cc`, `cmake` or `pkg-config`
// build-dependency) and links it: what a script does that drives swiftc, an assembler or a
// vendored toolchain on its own.
use std::{env, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let obj = out.join("add.o");
    let lib = out.join("libfixture_add.a");
    let cc = env::var("CC").unwrap_or_else(|_| "cc".into());
    assert!(Command::new(cc).args(["-c", "-fPIC", "native/add.c", "-o"]).arg(&obj).status().unwrap().success(), "cc failed");
    let _ = std::fs::remove_file(&lib);
    assert!(Command::new("ar").arg("rcs").arg(&lib).arg(&obj).status().unwrap().success(), "ar failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=fixture_add");
    println!("cargo:rerun-if-changed=native/add.c");
}
