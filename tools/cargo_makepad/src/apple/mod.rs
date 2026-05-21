mod compile;
mod sdk;
use crate::utils::{get_build_crate_from_args, get_package_binary_name};
use compile::*;

#[allow(dead_code)]
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug)]
pub enum AppleTarget {
    aarch64_ios,
    aarch64_ios_sim,
    x86_64_ios_sim,
    aarch64_tvos,
    aarch64_tvos_sim,
    x86_64_tvos_sim,
}

impl AppleTarget {
    fn needs_build_std(&self) -> bool {
        match self {
            Self::aarch64_tvos | Self::aarch64_tvos_sim | Self::x86_64_tvos_sim => true,
            _ => false,
        }
    }
    /// rustup channel used to build this target.
    ///
    /// tier-3 targets (tvOS) require `-Z build-std`, which is nightly-only, so
    /// they always resolve to `nightly` regardless of `prefer_stable`. tier-2
    /// targets (iOS) have prebuilt std and build on `stable` unless the caller
    /// explicitly opted into nightly (`--nightly`).
    fn rust_channel(&self, prefer_stable: bool) -> &'static str {
        if self.needs_build_std() || !prefer_stable {
            "nightly"
        } else {
            "stable"
        }
    }
    fn os(&self) -> AppleOs {
        match self {
            Self::aarch64_tvos | Self::aarch64_tvos_sim | Self::x86_64_tvos_sim => AppleOs::Tvos,
            Self::aarch64_ios | Self::aarch64_ios_sim | Self::x86_64_ios_sim => AppleOs::Ios,
        }
    }
}

#[derive(Copy, Clone)]
pub enum AppleOs {
    Ios,
    Tvos,
}

impl AppleTarget {
    fn toolchain(&self) -> &'static str {
        match self {
            Self::aarch64_ios => "aarch64-apple-ios",
            Self::aarch64_ios_sim => "aarch64-apple-ios-sim",
            Self::x86_64_ios_sim => "x86_64-apple-ios",
            Self::aarch64_tvos => "aarch64-apple-tvos",
            Self::aarch64_tvos_sim => "aarch64-apple-tvos-sim",
            Self::x86_64_tvos_sim => "x86_64-apple-tvos",
        }
    }
    fn device_target(os: AppleOs) -> Self {
        match os {
            AppleOs::Ios => Self::aarch64_ios,
            AppleOs::Tvos => Self::aarch64_tvos,
        }
    }
    fn sim_target(os: AppleOs) -> Self {
        #[cfg(target_arch = "x86_64")]
        match os {
            AppleOs::Ios => Self::x86_64_ios_sim,
            AppleOs::Tvos => Self::x86_64_tvos_sim,
        }
        #[cfg(target_arch = "aarch64")]
        match os {
            AppleOs::Ios => Self::aarch64_ios_sim,
            AppleOs::Tvos => Self::aarch64_tvos_sim,
        }
    }
}

pub fn handle_apple(args: &[String]) -> Result<(), String> {
    let mut signing_identity = None;
    let mut provisioning_profile = None;
    let mut device_identifier = None;
    let mut app = None;
    let mut org = None;
    // Default to the stable toolchain. iOS targets build fine on stable; tvOS
    // targets transparently fall back to nightly (see AppleTarget::rust_channel)
    // because they need `-Z build-std`. Pass `--nightly` to force nightly.
    let mut stable = true;
    if args.len() < 1 {
        return Err(format!("not enough args"));
    }
    let apple_os = match args[0].as_ref() {
        "ios" => AppleOs::Ios,
        "tvos" => AppleOs::Tvos,
        "list" => {
            let pp = parse_profiles()?;
            pp.println();
            return Ok(());
        }
        _ => return Err(format!("please enter ios or tvos")),
    };
    let mut args = &args[1..];
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--cert=") {
            signing_identity = Some(opt.to_string());
        } else if let Some(opt) = v.strip_prefix("--profile=") {
            provisioning_profile = Some(opt.to_string());
        } else if let Some(opt) = v.strip_prefix("--device=") {
            device_identifier = Some(opt.to_string());
        } else if let Some(opt) = v.strip_prefix("--app=") {
            app = Some(opt.to_string());
        } else if let Some(opt) = v.strip_prefix("--org=") {
            org = Some(opt.to_string());
        } else if v == "--stable" {
            stable = true;
        } else if v == "--nightly" {
            stable = false;
        } else {
            args = &args[i..];
            break;
        }
    }

    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain" => {
            let toolchains = vec![
                AppleTarget::sim_target(apple_os),
                AppleTarget::device_target(apple_os),
            ];
            sdk::rustup_toolchain_install(&toolchains, stable)
        }
        "build" => {
            let build_crate = get_build_crate_from_args(&args[1..])?;
            let default_app =
                get_package_binary_name(build_crate).unwrap_or_else(|| build_crate.to_string());
            let apple_target = AppleTarget::sim_target(apple_os);
            let result = compile::build(
                stable,
                &org.unwrap_or("orgname".to_string()),
                &app.unwrap_or(default_app),
                &args[1..],
                apple_target,
            )?;
            compile::copy_resources(
                &result.app_dir,
                build_crate,
                &result.build_dir,
                apple_target,
            )?;
            Ok(())
        }
        "run-device" => {
            compile::run_on_device(
                AppleArgs {
                    stable,
                    _apple_os: apple_os,
                    signing_identity,
                    provisioning_profile,
                    device_identifier,
                    app,
                    org,
                },
                &args[1..],
                AppleTarget::device_target(apple_os),
            )?;
            Ok(())
        }
        "run-sim" => {
            compile::run_on_sim(
                AppleArgs {
                    stable,
                    _apple_os: apple_os,
                    signing_identity,
                    provisioning_profile,
                    device_identifier,
                    app,
                    org,
                },
                &args[1..],
                AppleTarget::sim_target(apple_os),
            )?;
            Ok(())
        }
        _ => Err(format!("{} is not a valid command or option", args[0])),
    }
}
