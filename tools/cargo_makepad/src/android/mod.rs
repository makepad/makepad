#![allow(unused)]
use makepad_miniz::zip_file::*;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    fs::File,
    fs,
    process::{Command, Child, Stdio}
};

const URL_PLATFORM_33: &'static str = "https://dl.google.com/android/repository/platform-33-ext4_r01.zip";

const URL_BUILD_TOOLS_33_MACOS: &'static str = "https://dl.google.com/android/repository/build-tools_r33.0.1-macosx.zip";
const URL_BUILD_TOOLS_33_LINUX: &'static str = "https://dl.google.com/android/repository/build-tools_r33.0.1-linux.zip";
const URL_BUILD_TOOLS_33_WINDOWS: &'static str = "https://dl.google.com/android/repository/build-tools_r33.0.1-windows.zip";

const URL_PLATFORM_TOOLS_33_MACOS: &'static str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip";
const URL_PLATFORM_TOOLS_33_LINUX: &'static str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-linux.zip";
const URL_PLATFORM_TOOLS_33_WINDOWS: &'static str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-windows.zip";

const URL_NDK_33_MACOS: &'static str = "https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg";
const URL_NDK_33_LINUX: &'static str = "https://dl.google.com/android/repository/android-ndk-r25c-linux.zip";
const URL_NDK_33_WINDOWS: &'static str = "https://dl.google.com/android/repository/android-ndk-r25c-windows.zip";

const URL_OPENJDK_17_0_2_WINDOWS_X64: &'static str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_windows-x64_bin.zip";
const URL_OPENJDK_17_0_2_MACOS_AARCH64: &'static str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-aarch64_bin.tar.gz";
const URL_OPENJDK_17_0_2_MACOS_X64: &'static str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-x64_bin.tar.gz";
const URL_OPENJDK_17_0_2_LINUX_X64: &'static str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_linux-x64_bin.tar.gz";

#[derive(Clone, Copy,PartialEq)]
enum HostOs {
    WindowsX64,
    MacosX64,
    MacosAarch64,
    LinuxX64,
    Unsupported
}

impl HostOs {
    fn from_str(opt:&str)->Result<Self, String>{
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
        match self{
            Self::WindowsX64=>"./android_33_windows_x64",
            Self::MacosX64=> "./android_33_macos_x64",
            Self::MacosAarch64=>"./android_33_macos_aarch64",
            Self::LinuxX64=>"./android_33_linux_x64",
            Self::Unsupported=>panic!()
        }
    }
}

fn shell(cwd: &Path, cmd: &str, args: &[&str]) -> Result<(), String> {
    let mut cmd_build = Command::new(cmd);
    
    cmd_build.args(args)
        .current_dir(cwd);
    
    let mut child = cmd_build.spawn().map_err( | e | format!("Error starting {} in dir {:?} - {:?}", cmd, cwd, e)) ?;
    
    let r = child.wait().map_err( | e | format!("Process {} in dir {:?} returned error {:?} ", cmd, cwd, e)) ?;
    if !r.success() {
        return Err(format!("Process {} in dir {:?} returned error exit code ", cmd, cwd));
    }
    Ok(())
}

fn mkdir(path: &Path) -> Result<(), String> {
    match fs::create_dir_all(path) {
        Err(e) => {
            Err(format!("mkdir {:?} failed {:?}", path, e))
        },
        Ok(()) => Ok(())
    }
}

fn cp_r(source:&Path, dest:&Path){
    // lets copy source path to dest path recursive
    
}

fn url_file_name(url:&str)->&str{
    url.rsplit_once("/").unwrap().1
}

fn download_full_sdk(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<(), String> {
    // get current working directory
    
    mkdir(sdk_dir) ?;
    
    fn curl(step: usize, sdk_dir: &Path, url: &str) -> Result<(), String> {
        println!("---- Downloading {step}/5: {} ----", url);
        shell(&sdk_dir, "curl", &[url, "--output", sdk_dir.join(url_file_name(url)).to_str().unwrap()]) ?;
        Ok(())
    }
    curl(1, sdk_dir, URL_PLATFORM_33) ?;
    match host_os {
        HostOs::WindowsX64 => {
            curl(2, sdk_dir, URL_BUILD_TOOLS_33_WINDOWS) ?;
            curl(3, sdk_dir, URL_PLATFORM_TOOLS_33_WINDOWS) ?;
            curl(4, sdk_dir, URL_NDK_33_WINDOWS) ?;
            curl(5, sdk_dir, URL_OPENJDK_17_0_2_WINDOWS_X64) ?;
        }
        HostOs::MacosX64 | HostOs::MacosAarch64 => {
            curl(2, sdk_dir, URL_BUILD_TOOLS_33_MACOS) ?;
            curl(3, sdk_dir, URL_PLATFORM_TOOLS_33_MACOS) ?;
            curl(4, sdk_dir, URL_NDK_33_MACOS) ?;
            if host_os == HostOs::MacosX64 {
                curl(5, sdk_dir, URL_OPENJDK_17_0_2_MACOS_X64) ?;
            }
            else {
                curl(5, &sdk_dir, URL_OPENJDK_17_0_2_MACOS_AARCH64) ?;
            }
        }
        HostOs::LinuxX64 => {
            curl(2, sdk_dir, URL_BUILD_TOOLS_33_LINUX) ?;
            curl(3, sdk_dir, URL_PLATFORM_TOOLS_33_LINUX) ?;
            curl(4, sdk_dir, URL_NDK_33_LINUX) ?;
            curl(5, sdk_dir, URL_OPENJDK_17_0_2_LINUX_X64) ?;
        }
        HostOs::Unsupported=>panic!()
    }
    println!("All Android SDK files downloaded in {:?}\nAs the next step, run: cargo makepad android expand-full-sdk", sdk_dir);
    // alright lets parse the sdk_path option
    Ok(())
}

// ok lets parse some zip files for the fun of it so we can install on windows without too much shit


fn expand_full_sdk(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<(), String> {
    
    fn unzip(step: usize, sdk_dir: &Path, url: &str) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Unzipping {step}/5: {} ----", url_file_name);
        // lets open this file
        let mut file = File::open(sdk_dir.join(url_file_name))
            .map_err(|e| format!("Cant open file {url_file_name}"))?;
        
        let directory = zip_read_central_directory(&mut file)
            .map_err(|e| format!("Can't read zipfile {url_file_name} {:?}", e))?;
        
        // alright we have a directory. lets dump it
        
        if let Some(file_header) = directory.file_headers.iter().find(|v| v.file_name == "android-33-ext4/android.jar"){
            let data = file_header.extract(&mut file).map_err(|e| format!("Can't extract file from {url_file_name} {:?}", e))?;
            println!("GOT FILE {}", data.len());
        }
        
        Ok(())
    }
    
    fn untar(step: usize, sdk_dir: &Path, url: &str) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Untarring {step}/5: {} ----", url_file_name);
        shell(&sdk_dir, "tar", &["-xf", sdk_dir.join(url_file_name).to_str().unwrap()]) ?;
        Ok(())
    }
    
    fn dmg_extract(step: usize, sdk_dir: &Path, url: &str, src_dir:&str, dst_dir:&str) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Mounting and extracting dmg {step}/5: {} ----", url_file_name);
        let mount_point = &sdk_dir.join(&format!("mount_{url_file_name}"));
        mkdir(mount_point)?;
        shell(&sdk_dir, "hdiutil", &["attach", "-quiet", "-mountpoint", mount_point.to_str().unwrap(), sdk_dir.join(url_file_name).to_str().unwrap()]) ?;
        //shell(&sdk_dir, "umount", &[mount_point.to_str().unwrap()])?;
        Ok(())
    }
    
    match host_os {
        HostOs::WindowsX64 => {
        }
        HostOs::MacosX64 | HostOs::MacosAarch64 => {
            unzip(1, sdk_dir, URL_PLATFORM_33)?;
            /*unzip(1, sdk_dir, URL_PLATFORM_33); 
            unzip(2, sdk_dir, URL_BUILD_TOOLS_33_MACOS);
            unzip(3, sdk_dir, URL_PLATFORM_TOOLS_33_MACOS);
            // mount the dmg and copy the contents out
            dmg_extract(4, sdk_dir, URL_NDK_33_MACOS, "AndroidNDK9519653.app/Contents/NDK","NDK");
            if host_os == HostOs::MacosX64 {
                untar(5, sdk_dir, URL_OPENJDK_17_0_2_MACOS_X64) ?;
            }
            else {
                untar(5, &sdk_dir, URL_OPENJDK_17_0_2_MACOS_AARCH64) ?;
            }*/
        }
        HostOs::LinuxX64 => {
        }
        HostOs::Unsupported=>panic!()
    }
    Ok(())
}

pub fn handle_android(mut args: &[String]) -> Result<(), String> {

    let mut host_os = HostOs::Unsupported;
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))] let mut host_os = HostOs::WindowsX64;
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))] let mut host_os = HostOs::MacosX64;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))] let mut host_os = HostOs::MacosAarch64;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))] let mut host_os = HostOs::LinuxX64;
    let mut sdk_path = None;
    // pull out options
    for i in 0..args.len() {
        let v = &args[i];
        if let Some(opt) = v.strip_prefix("--host-os=") {
            host_os = HostOs::from_str(opt)?;
        }
        else if let Some(opt) = v.strip_prefix("--sdk-path=") {
            sdk_path = Some(opt.to_string());
        }
        else {
            args = &args[i..];
            break
        }
    }
    if sdk_path.is_none(){
        sdk_path = Some(host_os.default_path().to_string());
    }
    
    let mut cwd = std::env::current_dir().unwrap();
    let sdk_dir = cwd.join(&sdk_path.unwrap());
    
    match args[0].as_ref() {
        "download-full-sdk" => {
            return download_full_sdk(&sdk_dir, host_os, &args[1..])
        }
        "expand-full-sdk" => {
            return expand_full_sdk(&sdk_dir, host_os, &args[1..])
        }
        "install-full-sdk" => {
            download_full_sdk(&sdk_dir, host_os, &args[1..]) ?;
            expand_full_sdk(&sdk_dir, host_os, &args[1..])
        }
        _ => Err(format!("{} is not a valid command or option", args[0]))
    }
}
