use crate::apple::AppleTarget;
use crate::makepad_shell::*;

pub fn rustup_toolchain_install(
    apple_targets: &[AppleTarget],
    prefer_stable: bool,
) -> Result<(), String> {
    println!("[Begin] Installing Rust toolchains for Apple devices");
    let cwd = std::env::current_dir().unwrap();

    for target in apple_targets {
        // Each target picks its own channel: iOS -> stable (unless --nightly),
        // tvOS -> always nightly (needs `-Z build-std`). Only install the
        // channel if it's genuinely missing — never force-update it.
        let channel = target.rust_channel(prefer_stable);
        crate::utils::ensure_rust_toolchain_installed(channel)?;

        if target.needs_build_std() {
            // tier-3 target (tvOS): no prebuilt std. `-Z build-std` compiles
            // std from source at build time, which needs the `rust-src`
            // component rather than a `rustup target add`.
            shell_env(
                &[],
                &cwd,
                "rustup",
                &["component", "add", "rust-src", "--toolchain", channel],
            )?;
        } else {
            // tier-2 target (iOS): prebuilt std fetched via `rustup target add`.
            shell_env(
                &[],
                &cwd,
                "rustup",
                &["target", "add", target.toolchain(), "--toolchain", channel],
            )?;
        }
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
