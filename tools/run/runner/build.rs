//! The bare runner's build: RELAX_RUNNER_PROVENANCE (build_stamp.rs: content hashes of the packages
//! this binary compiles, never git state). Provides no crate (tools/run/build.rs does that for `makepad`).
#[path = "../../../libs/relax/base/src/json.rs"]
#[allow(dead_code)]
mod json;
#[path = "../../../libs/relax/base/src/srchash.rs"]
#[allow(dead_code)]
mod srchash;
#[path = "../../../libs/relax/build_stamp.rs"]
mod build_stamp;
use build_stamp::vfs;

fn main() {
    // Windows: Relax runs the program's main on the process's main thread; give it the 8 MiB
    // macOS and Linux main threads have (a PE's default reserve is 1-2 MiB)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        } else if std::env::var("CARGO_ENCODED_RUSTFLAGS").is_ok_and(|f| f.contains("linker-flavor=ld.lld")) {
            println!("cargo:rustc-link-arg-bins=--stack=8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        }
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../../libs/relax/base/src/json.rs");
    println!("cargo:rerun-if-changed=../../../libs/relax/base/src/srchash.rs");
    println!("cargo:rerun-if-changed=../../../libs/relax/build_stamp.rs");
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let g = build_stamp::graph(&dir);
    println!("cargo:rustc-env=RELAX_RUNNER_PROVENANCE={}", g.provenance(&[&dir.join("../../../libs/relax"), &dir], ""));
}
