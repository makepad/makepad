use crate::makepad_shell::*;
use crate::apple::{AppleTarget};

pub fn rustup_toolchain_install(apple_targets:&[AppleTarget]) -> Result<(), String> {
    println!("[Begin] Installing Rust toolchains for Apple devices");
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    
    for target in apple_targets{
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
            "target",
            "add",
            target.toolchain(),
            "--toolchain",
            "nightly"
            ]) ?;
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "component",
                "add",
                "rust-std",
                "--target",
                target.toolchain(),
                "--toolchain",
                "nightly"
            ]) ?;
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
                "component",
                "add",
                "rust-std",
                "--target",
                target.toolchain(),
                "--toolchain",
                "stable"
            ]) ?;
    }
    /*
    let cwd = std::env::current_dir().unwrap();
    let ios_deploy_dir = cwd.join(format!("{}/ios-deploy", env!("CARGO_MANIFEST_DIR")));
    
    shell_env_cap(&[],&ios_deploy_dir, "xcodebuild", &[
        "-quiet",
        "-target",
        "ios-deploy",  
    ]) ?;
    */
    println!("[Finished] Apple Rust toolchains installed");
    Ok(())
}
