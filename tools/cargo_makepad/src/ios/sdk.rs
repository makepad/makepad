use crate::shell::*;
use crate::ios::{IosTarget};

pub fn rustup_toolchain_install(ios_targets:&[IosTarget]) -> Result<(), String> {
    println!("[Begin] Installing Rust toolchains for iOS");
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
    
    let cwd = std::env::current_dir().unwrap();
    let ios_deploy_dir = cwd.join(format!("{}/ios-deploy", env!("CARGO_MANIFEST_DIR")));
    
    shell_env_cap(&[],&ios_deploy_dir, "xcodebuild", &[
        "-quiet",
        "-target",
        "ios-deploy",
    ]) ?;
    
    println!("[Finished] iOS Rust toolchains installed");
    Ok(())
}
