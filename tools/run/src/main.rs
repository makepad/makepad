//! The `makepad` binary: the runner (relax_runner) with this binary's host (lib.rs). On iOS it is
//! the app bundle's executable and runs the bundle's packed app (ios.rs).

fn main() {
    #[cfg(target_os = "ios")]
    makepad_run::ios::main();
    #[cfg(not(target_os = "ios"))]
    relax_runner::main(&makepad_run::HOST)
}
