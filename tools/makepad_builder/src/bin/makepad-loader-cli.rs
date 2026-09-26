fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Rust's GNU toolchain asks `dlltool` for the import libraries of
    // raw-dylib DLLs; for that the Builder puts a copy of itself on the
    // build's PATH under that name (see implib.rs).
    let invoked_as = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()));
    if invoked_as.as_deref() == Some("dlltool") {
        if let Err(e) = makepad_loader::implib::dlltool(&args[1..]) {
            eprintln!("dlltool: {e}");
            std::process::exit(1);
        }
        return;
    }
    // Started without arguments (double-clicked, or from the .bat): the TUI.
    let result = if args.len() == 1 { makepad_loader::tui::run() } else { makepad_loader::cli_main() };
    if let Err(e) = result {
        eprintln!("makepad-builder: {e}");
        if args.len() == 1 {
            // A window Explorer opened would close before this could be read.
            eprintln!("Press Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
        }
        std::process::exit(1);
    }
}
