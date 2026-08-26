// WINDOWS MAIN-THREAD STACK RESERVE.
//
// Windows gives the main thread a 1 MB stack by default; macOS gives 8 MB and
// Linux 8 MB. The VJ app's startup walks a very large script tree — fourteen
// script_mod registration passes (widgets, render, xr, asset widgets, chat ui,
// views, mesh, flow warp, nv12, music, effects, fx thumbs, fx slot, midi learn)
// plus 261 bundled effect presets — and the DSL evaluator is a recursive
// descent over that tree. Peak main-thread usage measured between 1 MB and
// 2 MB: bounded, but just over what Windows hands out. The result on Windows
// was a hard `thread 'main' has overflowed its stack` at boot, before the
// window ever appeared.
//
// Verified by bracketing the macOS binary with `ulimit -s`: at 1024 KB it
// reproduces the Windows crash exactly (same final log lines), at 2048 KB it
// boots and runs. So this is a platform default, not a runaway recursion —
// raising the reserve is the fix, not a paper-over.
//
// /STACK sets the PE header's stack RESERVE, which is address space only;
// pages commit lazily, so a 16 MB reserve costs no real memory and simply
// gives the same headroom the other platforms already have.
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
    }
}
