mod compile;
mod sdk;

#[derive(Clone, Copy, PartialEq)]
pub enum HostOs {
    WindowsX64,
    MacosX64,
    MacosAarch64,
    LinuxX64,
    Unsupported
}

#[allow(non_camel_case_types)]
pub enum AndroidTarget {
    aarch64,
    x86_64,
    armv7,
    i686
}

impl AndroidTarget {
    fn from_str(opt: &str) -> Result<Vec<Self>,
    String> {
        let mut out = Vec::new();
        for opt in opt.split(","){
            match opt {
                "all"=> return Ok(vec![AndroidTarget::aarch64, AndroidTarget::x86_64, AndroidTarget::armv7, AndroidTarget::i686]),
                "aarch64" => out.push(AndroidTarget::aarch64),
                "x86_64" => out.push(AndroidTarget::x86_64),
                "armv7" => out.push(AndroidTarget::armv7),
                "i686" => out.push(AndroidTarget::i686),
                x => {
                    return Err(format!("{:?} please provide a valid target: aarch64, x86_64, armv7, i686", x))
                }
            }
        }
        return Ok(out);
    }
    fn sys_dir(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android",
            Self::armv7 => "arm-linux-androideabi",
            Self::i686 => "i686-linux-android",
        }
    }
    fn clang(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android", 
            Self::armv7 => "armv7a-linux-androideabi", 
            Self::i686 => "i686-linux-android", 
        }
    }
    fn toolchain(&self)->&'static str{
        match self {
            Self::aarch64 => "aarch64-linux-android",
            Self::x86_64 => "x86_64-linux-android",
            Self::armv7 => "armv7-linux-androideabi",
            Self::i686 => "i686-linux-android"
        }
    }
    fn to_str(&self) -> &'static str {
        match self {
            Self::aarch64 => "aarch64",
            Self::x86_64 => "x86_64",
            Self::armv7 => "armv7",
            Self::i686 => "i686",
        }
    }
    fn abi_identifier(&self) -> &'static str {
        match self {
            Self::aarch64 => "arm64-v8a",
            Self::x86_64 => "x86_64",
            Self::armv7 => "armeabi-v7a",
            Self::i686 => "x86",
        }
    }
    fn linker_env_var(&self) -> &'static str {
        match self {
            Self::aarch64 => "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER",
            Self::x86_64 => "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER",
            Self::armv7 => "CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER",
            Self::i686 => "CARGO_TARGET_I686_LINUX_ANDROID_LINKER",
        }
    }
}

impl HostOs {
    fn from_str(opt: &str) -> Result<Self,
    String> {
        match opt {
            "windows-x64" => Ok(HostOs::WindowsX64),
            "macos-x64" => Ok(HostOs::MacosX64),
            "macos-aarch64" => Ok(HostOs::MacosAarch64),
            "linux-x64" => Ok(HostOs::LinuxX64),
            x => {
                Err(format!("{:?} please provide a valid host-os: windows-x64,macos-x64,macos-aarch64,linux-x64", x))
            }
        }
    }
    
    fn default_path(&self) -> &'static str {
        match self {
            Self::WindowsX64 => "./android_33_windows_x64",
            Self::MacosX64 => "./android_33_macos_x64",
            Self::MacosAarch64 => "./android_33_macos_aarch64",
            Self::LinuxX64 => "./android_33_linux_x64",
            Self::Unsupported => panic!()
        }
    }
}

pub fn handle_android(mut args: &[String]) -> Result<(), String> {
    #[allow(unused)]
    let mut host_os = HostOs::Unsupported;
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))] let mut host_os = HostOs::WindowsX64;
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))] let mut host_os = HostOs::MacosX64;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))] let mut host_os = HostOs::MacosAarch64;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))] let mut host_os = HostOs::LinuxX64;
    let mut sdk_path = None;
    let mut package_name = None;
    let mut app_label = None;
    let mut targets = vec![AndroidTarget::aarch64];
    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--host-os=") {
            host_os = HostOs::from_str(opt) ?;
        }
        else if let Some(opt) = v.strip_prefix("--sdk-path=") {
            sdk_path = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--package-name=") {
            package_name = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--app-label=") {
            app_label = Some(opt.to_string());
        }
        else if let Some(opt) = v.strip_prefix("--target=") {
            targets = AndroidTarget::from_str(opt)?;
        }
        else {
            args = &args[i..];
            break
        }
    }
    if sdk_path.is_none() {
        sdk_path = Some(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), host_os.default_path().to_string()));
    }
    
    let cwd = std::env::current_dir().unwrap();
    let sdk_dir = cwd.join(sdk_path.unwrap());
    
    match args[0].as_ref() {
        "adb" => {
            compile::adb(&sdk_dir, host_os, &args[1..])
        },
        "java" => {
            compile::java(&sdk_dir, host_os, &args[1..])
        },
        "javac" => {
            compile::javac(&sdk_dir, host_os, &args[1..])
        },
        "rustup-toolchain-install" => {
            sdk::rustup_toolchain_install(&targets)
        }
        "download-sdk" => {
            sdk::download_sdk(&sdk_dir, host_os, &args[1..])
        }
        "expand-sdk" => {
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..], &targets)
        }
        "remove-sdk-sources" => {
            sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..])
        }
        "toolchain-install" => {
            println!("Installing Android toolchain\n");
            sdk::rustup_toolchain_install(&targets) ?;
            sdk::download_sdk(&sdk_dir, host_os, &args[1..]) ?;
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..], &targets) ?;
            sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..]) ?;
            println!("\nAndroid toolchain has been installed\n");
            Ok(())
        }
        /*"base-apk"=>{
            compile::base_apk(&sdk_dir, host_os, &args[1..])
        }*/
        "build" => {
            compile::build(&sdk_dir, host_os, package_name, app_label, &args[1..], &targets) ?;
            Ok(())
        }
        "run" => {
            compile::run(&sdk_dir, host_os, package_name, app_label, &args[1..], &targets)
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
