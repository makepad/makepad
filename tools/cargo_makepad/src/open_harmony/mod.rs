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

#[allow(non_camel_case_types)]
pub enum OpenHarmonyTarget {
    aarch64,
}

impl OpenHarmonyTarget {
    fn toolchain(&self) ->&'static str{
        match self {
            Self::aarch64 => "aarch64-unknown-linux-ohos"
        }
    }
}

pub fn handle_open_harmony(mut args: &[String]) -> Result<(), String> {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))] let host_os = HostOs::WindowsX64;
    #[cfg(all(target_os = "macos"))] let host_os = HostOs::MacOS;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))] let host_os = HostOs::LinuxX64;
    let targets = vec![OpenHarmonyTarget::aarch64];
    let mut deveco_home = None;
    let mut hdc_remote = None;

    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--deveco-home=") {
            deveco_home = Some(opt.to_string());
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

