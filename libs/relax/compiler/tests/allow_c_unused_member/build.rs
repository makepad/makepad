// Compiles csrc/*.c with the system C compiler ($CC or cc) into one static library in OUT_DIR, one
// member per file, as the cc crate would, and links it (`twice`: with a second copy of one member).
use std::process::Command;
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let lib = format!("{out}/libacu.a");
    let _ = std::fs::remove_file(&lib);
    let mut ar = Command::new("ar");
    ar.args(["crs", &lib]);
    for name in ["used", "unused", "needed", "helper"] {
        let obj = format!("{out}/{name}.o");
        assert!(Command::new(&cc).args(["-O2", "-fPIC", "-c", &format!("csrc/{name}.c"), "-o", &obj]).status().expect("cc").success(), "cc failed");
        ar.arg(&obj);
        println!("cargo:rerun-if-changed=csrc/{name}.c");
    }
    // `--features twice`: the archive holds the program's member twice, as a merged archive does
    // (the copy under another name): a link that takes every member fails on its definitions
    if std::env::var_os("CARGO_FEATURE_TWICE").is_some() {
        let copy = format!("{out}/used_copy.o");
        std::fs::copy(format!("{out}/used.o"), &copy).expect("copy used.o");
        ar.arg(&copy);
    }
    assert!(ar.status().expect("ar").success(), "ar failed");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=acu");
}
