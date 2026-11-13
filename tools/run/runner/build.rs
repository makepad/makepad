//! The bare runner's build: the Windows main-thread stack, and on Linux and Android a GNU
//! build-id note (a hash of the linked binary), by which `relax --version` names the binary
//! (relax_compiler::os::proc::binary_id; macOS's linker always writes its LC_UUID). Provides no
//! crate (tools/run/build.rs does that for `makepad`).

fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Windows: Relax runs the program's main on the process's main thread; give it the 8 MiB
    // macOS and Linux main threads have (a PE's default reserve is 1-2 MiB)
    if os == "windows" {
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        }
    }
    if os == "linux" || os == "android" {
        println!("cargo:rustc-link-arg-bins=-Wl,--build-id=sha1");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
