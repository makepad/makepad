// `makepad run relax-fork-busy`: os_fork_busy.rs (libs/relax/compiler/tests/std's, copied) as a program, so
// the forks run with the whole `makepad run` runtime around them (owner thread, ahead and O3
// workers, inspect server, flight recorder, source watcher). Driven by fork_busy.sh.
// (its #[test] fns and what only they use are not part of the program)
// Unix only: the forks are std's unix process API (CommandExt::pre_exec); Windows has none.
#[cfg(unix)]
#[allow(dead_code)]
mod body {
    include!("os_fork_busy.rs");
}

#[cfg(unix)]
fn main() {
    body::fork_while_jit_busy();
    body::concurrent_spawns_while_tiers_build();
    body::spawns_never_wait_for_each_others_children();
}

#[cfg(not(unix))]
fn main() {
    eprintln!("relax-fork-busy: it forks with pre_exec, which only unix has");
    std::process::exit(2);
}
