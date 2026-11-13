// Run by `makepad run --allow-c` (main) and `makepad test --allow-c` (its unit tests): the program
// calls its native package, makes the C compiler unusable (the fixture's cc refuses once the
// COMPILER_GONE file exists) and starts its own executable with a cleared environment. That child
// is the program as built, as cargo's is: it needs no C compiler (relax issue 177).
fn native_value() {
    assert_eq!(sn_native::value(), 7);
}

fn start_child(args: &[&str]) {
    native_value();
    std::fs::write(std::env::var_os("COMPILER_GONE").expect("COMPILER_GONE"), b"gone").unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap()).args(args).env_clear().output().unwrap();
    assert!(child.status.success(), "the program's run of itself needed the C compiler: {}", String::from_utf8_lossy(&child.stderr));
}

fn main() {
    if std::env::args().any(|a| a == "--child") {
        native_value();
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
