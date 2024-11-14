use crate::makepad_shell::*;
use crate::open_harmony::OpenHarmonyTarget;

pub fn rustup_toolchain_install(openharmoy_targets:&[OpenHarmonyTarget]) -> Result<(), String> {
    println!("[Begin] Installing Rust toolchains for OpenHarmony devices");
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;

    for target in openharmoy_targets{
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
            "target",
            "add",
            target.toolchain(),
            "--toolchain",
            "nightly"
            ]) ?
    }
    println!("[Finished] OpenHarmony Rust toolchains installed");
    Ok(())
}
