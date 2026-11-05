//! The assembler dlltool runs, for Windows' GNU Rust (makepad.bat compiles it to `as.exe`).
//!
//! rustc makes the import libraries of raw-dylib crates with the `dlltool.exe` of rust-mingw, and
//! that runs `as` on the two small assembly files of each library. rust-mingw has no `as.exe`, so
//! rustc's own assembler does them: `as -o out.o in.s` becomes one global_asm! of that file.
use std::process::{exit, Command};

fn main() {
    let (mut out, mut input) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-o" {
            out = args.next();
        } else if !arg.starts_with('-') {
            input = Some(arg);
        }
    }
    let (Some(out), Some(input)) = (out, input) else {
        eprintln!("as: wants -o <object> <assembly>");
        exit(2);
    };
    let input = std::fs::canonicalize(&input).unwrap_or_else(|e| {
        eprintln!("as: {input}: {e}");
        exit(1);
    });
    let source = format!("{out}.rs");
    let text = format!("core::arch::global_asm!(include_str!(r#\"{}\"#), options(att_syntax));\n", input.display());
    if let Err(e) = std::fs::write(&source, text) {
        eprintln!("as: {source}: {e}");
        exit(1);
    }
    let done = Command::new("rustc").args(["--edition=2021", "--crate-type=lib", "--crate-name=assembly", "--emit=obj", "-o", &out, &source]).status();
    let _ = std::fs::remove_file(&source);
    match done {
        Ok(status) if status.success() => {}
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("as: rustc: {e}");
            exit(1);
        }
    }
}
