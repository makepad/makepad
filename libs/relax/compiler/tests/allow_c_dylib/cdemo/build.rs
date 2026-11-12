// Builds libdemo, a shared library, into DEMO_SDK_DIR (a folder outside the dynamic loader's search
// path, as an SDK installs one) and a static shim in OUT_DIR that calls it, with the system C
// compiler ($CC or cc) and ar; links both.
use std::process::Command;

fn run(cmd: &mut Command) {
    assert!(cmd.status().expect("start the tool").success(), "{cmd:?} failed");
}

fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    let sdk = std::env::var("DEMO_SDK_DIR").expect("DEMO_SDK_DIR: the folder libdemo is built into");
    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_string());
    std::fs::create_dir_all(&sdk).unwrap();
    if cfg!(target_os = "macos") {
        run(Command::new(&cc).args(["-dynamiclib", "-install_name", "@rpath/libdemo.dylib", "-o", &format!("{sdk}/libdemo.dylib"), "csrc/demo.c"]));
    } else {
        run(Command::new(&cc).args(["-shared", "-fPIC", "-o", &format!("{sdk}/libdemo.so"), "csrc/demo.c"]));
    }
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
