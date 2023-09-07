use crate::shell::*;
use crate::ios::{IosTarget};

pub fn rustup_toolchain_install(ios_targets:&[IosTarget]) -> Result<(), String> {
    println!("Installing Rust toolchains for ios");
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    for target in ios_targets{
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
            "target",
            "add",
            target.toolchain(),
            "--toolchain",
            "nightly"
        ]) ?
    }
    Ok(())
}
