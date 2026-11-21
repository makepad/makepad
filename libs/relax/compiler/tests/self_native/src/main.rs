// Run by `makepad run --allow-c` (main) and `makepad test --allow-c` (its unit tests): the program
// calls its native package, makes the C compiler unusable (the fixture's cc refuses once the
// COMPILER_GONE file exists) and starts its own executable with a cleared environment. That child
// is the program as built, as cargo's is: it needs no C compiler (relax issue 177). With
// SN_LIVE_READY it only calls its native package and stays until the live watcher has looked.
fn native_value() {
    assert_eq!(sn_native::value(), 7);
}

fn start_child(args: &[&str]) {
    native_value();
    std::fs::write(std::env::var_os("COMPILER_GONE").expect("COMPILER_GONE"), b"gone").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap()).args(args).env_clear().output().unwrap();
    assert!(child.status.success(), "the program's run of itself needed the C compiler: {}", String::from_utf8_lossy(&child.stderr));
}

/// Held until the live watcher has looked at the tree: its ready file (RELAX_LIVE_READY, named
/// again in SN_LIVE_READY), then the 2 s without compiles after which a program that never
/// presents takes edits (jit.rs STARTUP_QUIET_NS), the watcher's startup scan and the quiet a
/// change settles in (reload.rs STARTUP_SCAN_LOOK, LIVE_QUIET), with room to spare.
fn hold_for_the_watcher(ready: std::ffi::OsString) {
    let start = std::time::Instant::now();
    while !std::path::Path::new(&ready).exists() {
        assert!(start.elapsed() < std::time::Duration::from_secs(60), "the live watcher never started");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(4000));
}

fn main() {
    if std::env::args().any(|a| a == "--child") {
        native_value();
    } else if let Some(ready) = std::env::var_os("SN_LIVE_READY") {
        native_value();
        hold_for_the_watcher(ready);
        println!("ok");
    } else {
        start_child(&["--child"]);
        println!("ok");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn child() {
        super::native_value();
    }

    #[test]
    fn the_harness_run_of_itself_needs_no_compiler() {
        super::start_child(&["--exact", "tests::child"]);
    }
}
