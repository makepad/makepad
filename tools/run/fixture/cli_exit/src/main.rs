//! A command-line program that links the HAL and opens no window. `makepad run` hosts it (its run
//! window opens beside the load), and the process ends with the program's code all the same, as a
//! rustc build's does: a parent that runs it (a test through `CARGO_BIN_EXE_*`) waits for that code.
//!   no argument: main returns (0); `<code>`: std::process::exit(code); `panic`: a panic (101).
//!   RELAX_CLI_EXIT_BOOT_FAIL=1: its build script fails, so the boot fails (build.rs).
use makepad_hal as _;

fn main() {
    match std::env::args().nth(1).as_deref() {
        None => println!("relax-cli-exit: main returns"),
        Some("panic") => panic!("relax-cli-exit: a panic"),
        Some(code) => {
            eprintln!("relax-cli-exit: exit {code}");
            std::process::exit(code.parse().unwrap_or(2));
        }
    }
}

