//! The `makepad` binary: the runner (relax_runner) with this binary's host (lib.rs).

fn main() {
    relax_runner::main(&makepad_run::HOST)
}
