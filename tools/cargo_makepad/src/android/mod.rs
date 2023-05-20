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
        else {
            args = &args[i..];
            break
        }
    }
    if sdk_path.is_none() {
        sdk_path = Some(format!("{}/{}",env!("CARGO_MANIFEST_DIR"),host_os.default_path().to_string()));
    }
    
    let cwd = std::env::current_dir().unwrap();
    let sdk_dir = cwd.join(sdk_path.unwrap());
    
    match args[0].as_ref() {
        "rustup-toolchain-install"=>{
            sdk::rustup_toolchain_install()
        }
        "adb"=>{
            compile::adb(&sdk_dir, host_os, &args[1..])
        },
        "java"=>{
            compile::java(&sdk_dir, host_os, &args[1..])
        },
        "javac"=>{
            compile::javac(&sdk_dir, host_os, &args[1..])
        },
        "download-sdk" => {
            sdk::download_sdk(&sdk_dir, host_os, &args[1..])
        }
        "expand-sdk" => {
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..])
        }
        "remove-sdk-sources" => {
            sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..])
        }
        "toolchain-install" => {
            println!("Installing Android toolchain\n");
            sdk::rustup_toolchain_install()?;
            sdk::download_sdk(&sdk_dir, host_os, &args[1..]) ?;
            sdk::expand_sdk(&sdk_dir, host_os, &args[1..])?;
            sdk::remove_sdk_sources(&sdk_dir, host_os, &args[1..])?;
            println!("\nAndroid toolchain has been installed\n");
            Ok(())
        }
        /*"base-apk"=>{
            compile::base_apk(&sdk_dir, host_os, &args[1..])
        }*/
        "build" =>{
            compile::build(&sdk_dir, host_os, package_name, app_label, &args[1..])?;
            Ok(())
        }
        "run" =>{
            compile::run(&sdk_dir, host_os, package_name, app_label, &args[1..])
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
