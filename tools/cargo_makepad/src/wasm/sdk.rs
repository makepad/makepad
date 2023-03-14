use crate::shell::*;

pub fn rustup_toolchain_install() -> Result<(), String> {
    println!("Installing Rust toolchains for wasm");
    shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "target",
        "add",
        "aarch64-linux-android",
        "--toolchain",
        "nightly"
    ]) ?;
    Ok(())
}
