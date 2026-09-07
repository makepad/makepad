fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let Some(compiler) = arguments.next() else {
        eprintln!("Studio compiler guard requires a compiler command");
        std::process::exit(2);
    };
    let previous =
        std::env::var_os("MAKEPAD_STUDIO_PREVIOUS_RUSTC_WRAPPER").filter(|value| !value.is_empty());
    let mut command = if let Some(previous) = previous {
        let mut command = std::process::Command::new(previous);
        command.arg(compiler);
        command
    } else {
        std::process::Command::new(compiler)
    };
    // Preserve Cargo's configured target/compiler flags and wrapper chain.
    command.args(arguments).arg("-Dwarnings");
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => {
            eprintln!("Studio compiler guard: {error}");
            std::process::exit(1);
        }
    }
}
