//! One portable executable: graphical terminal host, or its setup child.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
mod app;
pub use app::makepad_widgets;
fn main() {
    if matches!(std::env::args().nth(1).as_deref(), Some("tui" | "rebuild" | "launch-scope" | "download-probe")) {
        makepad_loader::tui::attach_parent_console();
        if let Err(error) = makepad_loader::cli_main() {
            eprintln!("makepad-builder: {error}");
            std::process::exit(1);
        }
    } else {
        app::run();
    }
}
