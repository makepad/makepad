//! `relax`: the bare runner (internal dev tool; the product is `makepad run`). No crate is
//! provided and no main-thread host is registered: every crate of a program -- the script VM,
//! Splash, the shader compiler, the HAL -- is JIT-compiled from source, so an edit to any of
//! them reruns without a rustc rebuild. A HAL program runs the HAL's main loop itself, as its
//! rustc build does (makepad-platform app_main.rs).

fn main() {
    relax_runner::main(&relax_runner::BARE)
}
