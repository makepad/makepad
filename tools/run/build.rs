//! The loader's build: MAKEPAD_RUN_PROVENANCE (build_stamp.rs: content hashes of the packages this
//! binary compiles, never git state) and the source identity of each provided crate.
use std::path::PathBuf;

#[path = "../../libs/relax/base/src/json.rs"]
#[allow(dead_code)]
mod json;
#[path = "../../libs/relax/base/src/srchash.rs"]
#[allow(dead_code)]
mod srchash;
#[path = "../../libs/relax/build_stamp.rs"]
mod build_stamp;
use build_stamp::vfs;

/// The crates this binary provides (src/lib.rs registers them).
const PROVIDED: &[&str] = &[
    "relax-provided-fixture",
    "makepad-live-id",
    "makepad-script",
    "makepad-script-std",
    "makepad-script-census",
    "makepad-script-compute",
    "makepad-shader",
    "makepad-tween-script",
    "makepad-script-audio-aot",
    "makepad-hal",
];

fn main() {
    // Windows: Relax runs the program's main on the process's main thread; give it the 8 MiB
    // macOS and Linux main threads have (a PE's default reserve is 1-2 MiB)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        } else if std::env::var("CARGO_ENCODED_RUSTFLAGS").is_ok_and(|f| f.contains("linker-flavor=ld.lld")) {
            // Rust's LLD called directly in MinGW mode (the Makepad Builder's GNU toolchain)
            println!("cargo:rustc-link-arg-bins=--stack=8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        }
    }
    // ELF: the #[relax_api] entries (makepad-relax-api) sit in the `relax_api` section, reached
    // only through __start_/__stop_relax_api; LLD's default start-stop-gc would drop every
    // object's section that nothing else references
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if !matches!(os.as_str(), "windows" | "macos" | "ios") && std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-Wl,-z,nostart-stop-gc");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../libs/relax/base/src/json.rs");
    println!("cargo:rerun-if-changed=../../libs/relax/base/src/srchash.rs");
    println!("cargo:rerun-if-changed=../../libs/relax/build_stamp.rs");
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let g = build_stamp::graph(&dir);
    // the loader's closure holds the provided crates' closures: their files are watched here
    println!("cargo:rustc-env=MAKEPAD_RUN_PROVENANCE={}", g.provenance(&[&dir.join("../../libs/relax"), &dir], ""));
    // MAKEPAD_RUN_SRC_<crate>: the source identity (srchash.rs) of each provided crate as this build
    // compiles it (its resolved features and dependencies)
    for name in PROVIDED {
        let root = g.pkgs.iter().position(|p| p.name == *name).unwrap_or_else(|| panic!("{} is not a dependency", name));
        let manifest = srchash::closure_manifest(&g.pkgs, root);
        let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(format!("{name}.src.bin"));
        std::fs::write(output, srchash::encode_manifest(&manifest)).expect("write provided source manifest");
        for i in g.closure(root) {
            let p = &g.pkgs[i];
            if p.dir.contains("/.cargo/registry/") || p.dir.contains("/.cargo/git/") { continue; }
            // Directory watches catch additions, including an initially absent build.rs.
            println!("cargo:rerun-if-changed={}", p.dir);
            let key = format!("{}@{}", p.name.replace('-', "_"), p.version);
            for ((package, path), _) in &manifest {
                if (package == &key || package == &format!("host:{key}")) && !path.starts_with('\0') {
                    println!("cargo:rerun-if-changed={}", PathBuf::from(&p.dir).join(path).display());
                }
            }
        }
        println!("cargo:rustc-env=MAKEPAD_RUN_SRC_{}={:016x}", name.replace('-', "_"), srchash::manifest_hash(&manifest));
    }
}
