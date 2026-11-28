use crate::makepad_shell::*;

/// What a wasm build needs from the active toolchain: rust-src, as it compiles std for its own
/// target spec (`RUSTC_BOOTSTRAP=1`, no nightly), and the wasm32 target for plain checks.
pub fn rustup_toolchain_install() -> Result<(), String> {
    println!("Adding rust-src and the wasm32 target to the active Rust toolchain");
    let cwd = std::env::current_dir().unwrap();
    shell_env(&[], &cwd, "rustup", &["component", "add", "rust-src"])?;
    shell_env(&[], &cwd, "rustup", &["target", "add", "wasm32-unknown-unknown"])?;
    Ok(())
}
