fn main() {
    if let Err(e) = makepad_loader::cli_main() {
        eprintln!("makepad-loader: {e}");
        std::process::exit(1);
    }
}
