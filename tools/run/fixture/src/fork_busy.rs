// `makepad run relax-fork-busy`: libs/relax/compiler/tests/std/os_fork_busy.rs as a program, so
// the forks run with the whole `makepad run` runtime around them (owner thread, ahead and O3
// workers, inspect server, flight recorder, source watcher). Driven by fork_busy.sh.
// (its #[test] fns and what only they use are not part of the program)
#[allow(dead_code)]
mod body {
    include!("../../../../libs/relax/compiler/tests/std/os_fork_busy.rs");
}

fn main() {
    body::fork_while_jit_busy();
}
