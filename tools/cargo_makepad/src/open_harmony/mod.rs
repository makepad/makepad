use std::str::FromStr;

mod compile;
mod sdk;

#[allow(unused)]
#[derive(Clone, Copy, PartialEq)]
pub enum HostOs {
    WindowsX64,
    MacOS,
    LinuxX64,
    Unsupported
}

pub enum OpenHarmonyTarget {
    Aarch64,
    X86_64,
}
impl OpenHarmonyTarget {
    fn target_triple_str(&self) -> &'static str {
        match self {
            Self::Aarch64 => "aarch64-unknown-linux-ohos",
            Self::X86_64 => "x86_64-unknown-linux-ohos",
        }
    }
}
impl FromStr for OpenHarmonyTarget {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "aarch64" => Ok(OpenHarmonyTarget::Aarch64),
            "x86_64" => Ok(OpenHarmonyTarget::X86_64),
            _ => Err(())
        }
    }
}
impl OpenHarmonyTarget {
    fn from_current_target_arch() -> Result<Self, ()> {
        #[cfg(target_arch = "aarch64")] {
            Ok(Self::Aarch64)
        }
        #[cfg(target_arch = "x86_64")] {
            Ok(Self::X86_64)
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))] {
            Err(())
        }
    }
}

pub fn handle_open_harmony(mut args: &[String]) -> Result<(), String> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))] let host_os = HostOs::WindowsX64;
    #[cfg(all(target_os = "macos"))] let host_os = HostOs::MacOS;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))] let host_os = HostOs::LinuxX64;
    let mut targets = Vec::new();
    let mut deveco_home = None;
    let mut hdc_remote = None;

    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--deveco-home=") {
            deveco_home = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--arch=") {
            targets.push(
                OpenHarmonyTarget::from_str(opt.trim())
                    .map_err(|_| format!("Invalid --arch argument: '{}'", opt))?
            );
        }
        else if let Some(remote) = v.strip_prefix("--remote=") {
            hdc_remote = Some(remote.to_string());
        }
        else {
            args = &args[i..];
            break
        }
    }

    if deveco_home.is_none() {
        if let Ok(v) = std::env::var("DEVECO_HOME") {
            deveco_home = Some(v);
        }
    }

    if hdc_remote.is_none() {
        if let Ok(v) = std::env::var("HDC_REMOTE") {
            hdc_remote = Some(v);
        }
    }

    if targets.is_empty() {
        targets.push(
            OpenHarmonyTarget::from_current_target_arch()
                .inspect(|target| println!("Using current target arch: '{}'", target.target_triple_str()))
                .map_err(|_| format!("Current target arch is unsupported. Try passing in the '--arch' argument"))?
        );
    }

    if args.len() < 1 {
        return Err(format!("not enough args"))
    }
    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain" => {
            sdk::rustup_toolchain_install(&targets)
        }
        "deveco" => {
            compile::deveco(&deveco_home, &args[1..], &host_os, &targets)
        }
        "build" => {
            compile::build(&deveco_home, &args[1..], &host_os, &targets)
        }
        "run" => {
            compile::run(&deveco_home, &args[1..], &host_os, &targets, &hdc_remote)
        }
        "cdylib" => {
            compile::rust_build(&deveco_home, &host_os,&args[1..], &targets)
        }
        "hilog" => {
            compile::hilog(&deveco_home, &args[1..], &host_os, &hdc_remote)
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))

    }
}

