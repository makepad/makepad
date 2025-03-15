use crate::makepad_shell::*;
use crate::open_harmony::OpenHarmonyTarget;

pub fn rustup_toolchain_install(openharmoy_targets:&[OpenHarmonyTarget]) -> Result<(), String> {
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ])?;

    for target in openharmoy_targets{
        let target_triple = target.target_triple_str();
        println!("Installing Rust nightly toolchain for '{target_triple}'...");
        shell_env(
            &[],
            &std::env::current_dir().unwrap(),
            "rustup",
            &[
                "target",
                "add",
                target.target_triple_str(),
                "--toolchain",
                "nightly"
            ],
        )?;
    }
    println!("Finished installing OpenHarmony (ohos) Rust toolchains.");
    Ok(())
}
