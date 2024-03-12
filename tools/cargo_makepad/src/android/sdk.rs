#![allow(non_snake_case)]

use makepad_miniz::zip_file::*;
use std::{
    path::Path,
    fs::{File, OpenOptions},
    io::{Write, Read, Seek},
};

use crate::{
    android::*,
    makepad_shell::*,
};

pub const SDK_VERSION: &str = "33.0.1";
pub const API_LEVEL: &str = "android-33-ext4";
pub const BUILD_TOOLS_DIR: &str = "build-tools";
pub const PLATFORMS_DIR: &str = "platforms";

pub const _NDK_VERSION_MAJOR: &str = "25";
pub const _NDK_VERSION_MINOR: &str = "2";
pub const _NDK_VERSION_BUILD: &str = "9519653";
pub const NDK_VERSION_FULL:  &str = "25.2.9519653";

const URL_PLATFORM_33: &str = "https://dl.google.com/android/repository/platform-33-ext4_r01.zip";

const URL_BUILD_TOOLS_33_MACOS: &str = "https://dl.google.com/android/repository/build-tools_r33.0.1-macosx.zip";
const URL_BUILD_TOOLS_33_LINUX: &str = "https://dl.google.com/android/repository/build-tools_r33.0.1-linux.zip";
const URL_BUILD_TOOLS_33_WINDOWS: &str = "https://dl.google.com/android/repository/build-tools_r33.0.1-windows.zip";

const URL_PLATFORM_TOOLS_33_MACOS: &str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip";
const URL_PLATFORM_TOOLS_33_LINUX: &str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-linux.zip";
const URL_PLATFORM_TOOLS_33_WINDOWS: &str = "https://dl.google.com/android/repository/platform-tools_r33.0.3-windows.zip";

const URL_NDK_33_MACOS: &str = "https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg";
const URL_NDK_33_LINUX: &str = "https://dl.google.com/android/repository/android-ndk-r25c-linux.zip";
const URL_NDK_33_WINDOWS: &str = "https://dl.google.com/android/repository/android-ndk-r25c-windows.zip";

const URL_OPENJDK_17_0_2_WINDOWS_X64: &str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_windows-x64_bin.zip";
const URL_OPENJDK_17_0_2_MACOS_AARCH64: &str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-aarch64_bin.tar.gz";
const URL_OPENJDK_17_0_2_MACOS_X64: &str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-x64_bin.tar.gz";
const URL_OPENJDK_17_0_2_LINUX_X64: &str = "https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_linux-x64_bin.tar.gz";

fn url_file_name(url: &str) -> &str {
    url.rsplit_once('/').unwrap().1
}

pub fn rustup_toolchain_install(targets:&[AndroidTarget]) -> Result<(), String> {
    println!("Installing Rust toolchains for android");
    println!("Installing nightly");
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    for target in targets{
        let toolchain = target.toolchain();
        println!("Installing rust nightly for {}", toolchain);
        shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
            "target",
            "add",
            toolchain,
            "--toolchain",
            "nightly"
        ]) ?;
    }
    Ok(())
}

pub fn download_sdk(sdk_dir: &Path, host_os: HostOs, _args: &[String]) -> Result<(), String> {
    // get current working directory
    let src_dir = &sdk_dir.join("sources");
    mkdir(src_dir) ?;
    
    fn curl(step: usize, src_dir: &Path, url: &str) -> Result<(), String> {
        //let https = HttpsConnection::connect("https://makepad.dev","https");
        println!("{step}/5: Downloading: {}", url);
        shell(src_dir, "curl", &[url, "-#", "--output", src_dir.join(url_file_name(url)).to_str().unwrap()]) ?;
        Ok(())
    }
    curl(1, src_dir, URL_PLATFORM_33) ?;
    match host_os {
        HostOs::WindowsX64 => {
            curl(2, src_dir, URL_BUILD_TOOLS_33_WINDOWS) ?;
            curl(3, src_dir, URL_PLATFORM_TOOLS_33_WINDOWS) ?;
            curl(4, src_dir, URL_NDK_33_WINDOWS) ?;
            curl(5, src_dir, URL_OPENJDK_17_0_2_WINDOWS_X64) ?;
        }
        HostOs::MacosX64 | HostOs::MacosAarch64 => {
            curl(2, src_dir, URL_BUILD_TOOLS_33_MACOS) ?;
            curl(3, src_dir, URL_PLATFORM_TOOLS_33_MACOS) ?;
            curl(4, src_dir, URL_NDK_33_MACOS) ?;
            if host_os == HostOs::MacosX64 {
                curl(5, src_dir, URL_OPENJDK_17_0_2_MACOS_X64) ?;
            }
            else {
                curl(5, src_dir, URL_OPENJDK_17_0_2_MACOS_AARCH64) ?;
            }
        }
        HostOs::LinuxX64 => {
            curl(2, src_dir, URL_BUILD_TOOLS_33_LINUX) ?;
            curl(3, src_dir, URL_PLATFORM_TOOLS_33_LINUX) ?;
            curl(4, src_dir, URL_NDK_33_LINUX) ?;
            curl(5, src_dir, URL_OPENJDK_17_0_2_LINUX_X64) ?;
        }
        HostOs::Unsupported => panic!()
    }
    // alright lets parse the sdk_path option
    Ok(())
}

pub fn remove_sdk_sources(sdk_dir: &Path, _host_os: HostOs, _args: &[String]) -> Result<(), String> {
    let src_dir = &sdk_dir.join("sources");
    rmdir(src_dir) 
}

pub fn expand_sdk(sdk_dir: &Path, host_os: HostOs, args: &[String], targets:&[AndroidTarget]) -> Result<(), String> {
    let full_ndk = args.contains(&String::from("--full-ndk"));
    let src_dir = &sdk_dir.join("sources");
    
    fn unzip(step: usize, src_dir: &Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("{step}/5: Unzipping: {}", url_file_name);
        let mut zip_file = File::open(src_dir.join(url_file_name))
            .map_err( | _ | format!("Cant open file {url_file_name}")) ?;
        
        // Hahahah i parsed my own zipfile. That was easier than using someones bad abstraction.
        let directory = zip_read_central_directory(&mut zip_file)
            .map_err( | e | format!("Can't read zipfile {url_file_name} {:?}", e)) ?;
        //for file in &directory.file_headers{
        //    println!("{}", file.file_name);
        //}
        #[allow(unused)]
        fn extract_file(directory: &ZipCentralDirectory, zip_file: &mut File, file_name: &str, output_file: &Path, exec: bool) -> Result<(), String> {
            if let Some(file_header) = directory.file_headers.iter().find( | v | v.file_name == file_name) {
                let is_symlink = (file_header.external_file_attributes >> 16) & 0o120000 == 0o120000;
                
                let data = file_header.extract(zip_file).map_err( | e | {
                    format!("Can't extract file from {file_name} {:?}", e)
                }) ?;
                
                mkdir(output_file.parent().unwrap()) ?;
                
                if is_symlink {
                    let link_to = std::str::from_utf8(&data).unwrap().to_string();
                    #[cfg(any(target_os = "macos", target_os = "linux"))]
                    use std::os::unix::fs::symlink;
                    #[cfg(any(target_os = "macos", target_os = "linux"))]
                    symlink(link_to, output_file);
                }
                else {
                    let mut output = File::create(output_file)
                        .map_err( | _ | format!("Cant open output file {:?}", output_file)) ?;
                    
                    output.write(&data)
                        .map_err( | _ | format!("Cant write output file {:?}", output_file)) ?;
                    
                    #[cfg(any(target_os = "macos", target_os = "linux"))]
                    if exec {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(output_file, PermissionsExt::from_mode(0o744))
                            .map_err( | _ | format!("Cant set exec permissions {:?}", output_file)) ?;
                    }
                }
                
                Ok(())
            }
            else {
                
                Err(format!("File not found in zip {}", file_name))
            }
        }
        // ok so
        for (file_path, exec) in files {
            let (source_path, dest_path) = if let Some((a, b)) = file_path.split_once('|') {(a, b)}else {(*file_path, *file_path)};
            extract_file(&directory, &mut zip_file, source_path, &sdk_dir.join(dest_path), *exec) ?;
        }
        Ok(())
    }
    
    
    fn untar(step: usize, src_dir: &Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("{step}/5: Untarring: {}", url_file_name);
        shell(src_dir, "tar", &["-xf", src_dir.join(url_file_name).to_str().unwrap()]) ?;
        
        for (file_path, exec) in files {
            let (source_path, dest_path) = if let Some((a, b)) = file_path.split_once('|') {(a, b)}else {(*file_path, *file_path)};
            cp(&src_dir.join(source_path), &sdk_dir.join(dest_path), *exec) ?;
        }
        Ok(())
    }
    
    fn dmg_extract(step: usize, src_dir: &Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)], full_ndk: bool) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("{step}/5: Mounting and extracting {} dmg: {}", if full_ndk { "full" } else { "partial" }, url_file_name);
        
        let mount_point = &src_dir.join(&format!("mount_{url_file_name}"));
        mkdir(mount_point) ?;
        
        shell(src_dir, "hdiutil", &["attach", "-quiet", "-mountpoint", mount_point.to_str().unwrap(), src_dir.join(url_file_name).to_str().unwrap()]) ?;
        
        for (file_path, exec) in files {
            let (source_path, dest_path) = if let Some((a, b)) = file_path.split_once('|') {(a, b)}else {(*file_path, *file_path)};
            let copy_fn = if full_ndk { shell::cp_all } else { shell::cp };
            copy_fn(&mount_point.join(source_path), &sdk_dir.join(dest_path), *exec) ?;
        }
        shell(sdk_dir, "umount", &[mount_point.to_str().unwrap()]) ?;
        Ok(())
    }
    
    fn copy_map(base_in: &str, base_out: &str, file: &str) -> String {
        // allow an empty path to be properly passed in for either base.
        fn separator(base: &str) -> &str {
            if base.is_empty() { "" } else { "/" }
        }
        
        let sep_in = separator(base_in);
        let sep_out = separator(base_out);
        format!("{base_in}{sep_in}{file}|{base_out}{sep_out}{file}")
    }
    
    match host_os {
        HostOs::WindowsX64 => {
            unzip(1, src_dir, sdk_dir, URL_PLATFORM_33, &[
                (&copy_map("", PLATFORMS_DIR, &format!("{API_LEVEL}/android.jar")), false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_WINDOWS, &[
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "aapt.exe"), false),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "zipalign.exe"), false),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/apksigner.jar"), false),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/d8.jar"), false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_WINDOWS, &[
                ("platform-tools/adb.exe", false),
                ("platform-tools/AdbWinApi.dll", false),
                ("platform-tools/AdbWinUsbApi.dll", false),
            ]) ?;
            const NDK_IN: &str = "android-ndk-r25c/toolchains/llvm/prebuilt/windows-x86_64";
            let NDK_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/windows-x86_64");
            
            if full_ndk {
                // We only need to extract the contents of the `NDK_IN` directory within the `URL_NDK_33_LINUX` zip file,
                // and then copy that directory it into the proper `NDK_OUT` directory location.
                let cwd = std::env::current_dir().unwrap();
                let url_file_name = url_file_name(URL_NDK_33_WINDOWS);
                println!("4/5: Unzipping: {} (full NDK)", url_file_name);
                let ndk_out_path = sdk_dir.join(NDK_OUT);
                mkdir(&ndk_out_path)?;

                // Some shell environments on Windows are Linux-like (Git Bash, cygwin, mingw, etc),
                // and therefore support `unzip` and `cp`.
                let unzip_result = shell(
                    &cwd,
                    "unzip",
                    &[
                        "-q", // quiet
                        "-o", // overwrite existing files
                        src_dir.join(url_file_name).to_str().unwrap(),
                        &format!("{NDK_IN}/**/*"),
                        "-d", src_dir.to_str().unwrap(),
                    ]
                );
                if unzip_result.is_ok() {
                    shell(
                        &cwd,
                        "cp",
                        &[
                            "--force",
                            "--recursive",
                            "--preserve",
                            src_dir.join(NDK_IN).to_str().unwrap(),
                            ndk_out_path.parent().unwrap().to_str().unwrap(),
                        ]
                    ).unwrap();
                } else {
                    // If `unzip` failed, we're running on a true Windows shell (cmd, powershell),
                    // so we instead use `tar` (which is the BSD version of tar) to extract the zip file.
                    // Bonus: the BSD tar utility supports extracting files directly into NDK_OUT.
                    let num_path_components = Path::new(NDK_IN).iter().count();
                    shell(
                        &cwd,
                        "tar",
                        &[
                            "-x",
                            "-z",
                            "-f", src_dir.join(url_file_name).to_str().unwrap(),
                            "--strip-components", &num_path_components.to_string(),
                            "-C", ndk_out_path.to_str().unwrap(),
                            NDK_IN,
                        ]
                    ).unwrap();
                }
            } else {
                let mut ndk_extract = Vec::new();
                for target in targets{
                    let sys_dir = target.sys_dir();
                    let clang = target.clang();
                    let unwind_dir = target.unwind_dir();
                    let SYS_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let SYS_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let UNWIND_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/windows-x86_64/lib64/clang/14.0.7/lib/linux/{unwind_dir}");

                    ndk_extract.extend_from_slice(&[
                        (copy_map(NDK_IN, NDK_OUT, &format!("bin/{clang}33-clang")), false),
                        (copy_map(NDK_IN, NDK_OUT, &format!("bin/{clang}33-clang.cmd")), false),
                        (copy_map(NDK_IN, NDK_OUT, "bin/libwinpthread-1.dll"), false),
                        (copy_map(NDK_IN, NDK_OUT, "bin/libxml2.dll"), false),
                        (copy_map(NDK_IN, NDK_OUT, "bin/clang.exe"), false),
                        (copy_map(NDK_IN, NDK_OUT, "bin/ld.exe"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtbegin_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtend_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libc.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libGLESv2.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libm.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "liblog.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libEGL.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libdl.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libaaudio.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libandroid.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                        (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                        (format!("{UNWIND_IN}/libunwind.a|{SYS_OUT}/libunwind.a"), false),
                    ]);
                }
                let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
                unzip(4, src_dir, sdk_dir, URL_NDK_33_WINDOWS, &ndk_extract) ?;
            }

            // patch the .cmd file to stop complaining
            {
                let cmd_file_path = sdk_dir.join(format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android33-clang.cmd"));

                // Open the file for reading
                let mut ndk_cmd = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&cmd_file_path)
                    .map_err(|_| format!("Can't open file {:?}", cmd_file_path))?;

                // Read the file into a String
                let mut ndk_cmd_data = String::new();
                ndk_cmd.read_to_string(&mut ndk_cmd_data)
                    .map_err(|_| format!("Can't read file {:?}", cmd_file_path))?;

                // Replace occurrences of `"%1"` with `%1`
                let ndk_cmd_data_modified = ndk_cmd_data.replace("\"%1\"", "%1");

                // Write the modified String back to the file
                ndk_cmd.set_len(0).map_err(|_| "failed to truncate")?; // Truncate the file
                ndk_cmd.seek(std::io::SeekFrom::Start(0)).map_err(|_| "failed to seek")?; // Reset the cursor position
                ndk_cmd.write_all(ndk_cmd_data_modified.as_bytes())
                    .map_err(|_| format!("Can't write to file {:?}", cmd_file_path))?;
            }

            
            const JDK_IN: &str = "jdk-17.0.2";
            const JDK_OUT: &str = "openjdk";
            unzip(5, src_dir, sdk_dir, URL_OPENJDK_17_0_2_WINDOWS_X64, &[
                (&copy_map(JDK_IN, JDK_OUT, "bin/java.exe"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/jar.exe"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/javac.exe"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/jvm.cfg"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/jli.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/java.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/jimage.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/zip.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/net.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/nio.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/verify.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "bin/server/jvm.dll"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/modules"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/tzmappings"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/tzdb.dat"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.security"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/exempt_local.policy"), false),
            ]) ?;
        }
        HostOs::MacosX64 | HostOs::MacosAarch64 => {
            unzip(1, src_dir, sdk_dir, URL_PLATFORM_33, &[
                (&copy_map("", PLATFORMS_DIR, &format!("{API_LEVEL}/android.jar")), false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_MACOS, &[
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "aapt"), true),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "zipalign"), true),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/apksigner.jar"), false),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/d8.jar"), false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_MACOS, &[
                ("platform-tools/adb", true),
            ]) ?;
            const NDK_IN: &str = "AndroidNDK9519653.app/Contents/NDK/toolchains/llvm/prebuilt/darwin-x86_64";
            let NDK_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/darwin-x86_64");
            
            if full_ndk {
                let toolchain_dir = copy_map(NDK_IN, NDK_OUT, "");
                let files = [ (toolchain_dir.as_str(), false) ];
                dmg_extract(4, src_dir, sdk_dir, URL_NDK_33_MACOS, &files, full_ndk) ?;
                // We copied over the entire contents of `toolchains/llvm/prebuilt/darwin-x86_64`,
                // but we still need to make the files in `bin` actually executable.
                #[cfg(any(target_os = "macos", target_os = "linux"))] {
                    use std::os::unix::fs::PermissionsExt;
                    let bin_dir = sdk_dir.join(NDK_OUT).join("bin");
                    std::fs::read_dir(bin_dir)
                        .expect("failed to read NDK `bin/` dir: {bin_dir:?}")
                        .filter_map(|r| r.ok().and_then(|entry| {
                            let path = entry.path();
                            path.is_file().then_some(path)
                        }))
                        .for_each(|bin_file|
                            std::fs::set_permissions(&bin_file, PermissionsExt::from_mode(0o744))
                                .expect("failed to set exec permissions on {bin_file:?}")
                        );
                }
            } else {
                let mut ndk_extract = Vec::new();
                #[allow(non_snake_case)]
                for target in targets{
                    let sys_dir = target.sys_dir();
                    let clang = target.clang();
                    let unwind_dir = target.unwind_dir();
                    let SYS_IN = &format!("AndroidNDK9519653.app/Contents/NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let SYS_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let UNWIND_IN = &format!("AndroidNDK9519653.app/Contents/NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64/clang/14.0.7/lib/linux/{unwind_dir}"); 
                    ndk_extract.extend_from_slice(&[
                        (copy_map(NDK_IN, NDK_OUT, &format!("bin/{clang}33-clang")), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/clang"), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/ld"), true),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libxml2.2.9.13.dylib"), false),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libxml2.dylib"), false),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libxml2.2.9.13.dylib"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtbegin_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtend_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libc.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libGLESv2.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libm.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "liblog.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libEGL.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libdl.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libaaudio.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libandroid.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                        (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                        (format!("{UNWIND_IN}/libunwind.a|{SYS_OUT}/libunwind.a"), false),
                    ]);
                }
                let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
                dmg_extract(4, src_dir, sdk_dir, URL_NDK_33_MACOS, &ndk_extract, full_ndk) ?;
            }
            
            const JDK_IN: &str = "jdk-17.0.2.jdk/Contents/Home";
            const JDK_OUT: &str = "openjdk";
            untar(5, src_dir, sdk_dir, if host_os == HostOs::MacosX64 {URL_OPENJDK_17_0_2_MACOS_X64}else {URL_OPENJDK_17_0_2_MACOS_AARCH64}, &[
                (&copy_map(JDK_IN, JDK_OUT, "bin/java"), true),
                (&copy_map(JDK_IN, JDK_OUT, "bin/jar"), true),
                (&copy_map(JDK_IN, JDK_OUT, "bin/javac"), true),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjli.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/jvm.cfg"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/server/libjsig.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/server/libjvm.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/modules"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/tzdb.dat"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjava.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjimage.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libnet.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libnio.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libverify.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libzip.dylib"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.security"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/exempt_local.policy"), false),
            ]) ?;
        }
        HostOs::LinuxX64 => {
            unzip(1, src_dir, sdk_dir, URL_PLATFORM_33, &[
                (&copy_map("", PLATFORMS_DIR, &format!("{API_LEVEL}/android.jar")), false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_LINUX, &[
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "aapt"), true),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib64/libc++.so"), true),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "zipalign"), true),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/apksigner.jar"), false),
                (&copy_map("android-13", &format!("{BUILD_TOOLS_DIR}/{SDK_VERSION}"), "lib/d8.jar"), false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_LINUX, &[
                ("platform-tools/adb", true),
            ]) ?;
            const NDK_IN: &str = "android-ndk-r25c/toolchains/llvm/prebuilt/linux-x86_64";
            let NDK_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/linux-x86_64");
            
            if full_ndk {
                // We only need to extract the contents of the `NDK_IN` directory within the `URL_NDK_33_LINUX` zip file,
                // and then copy that directory it into the proper `NDK_OUT` directory location.
                let cwd = std::env::current_dir().unwrap();
                let url_file_name = url_file_name(URL_NDK_33_LINUX);
                println!("4/5: Unzipping: {} (full NDK)", url_file_name);
                let ndk_out_path = sdk_dir.join(NDK_OUT);
                mkdir(&ndk_out_path)?;
                shell(
                    &cwd,
                    "unzip",
                    &[
                        "-q", // quiet
                        "-o", // overwrite existing files
                        src_dir.join(url_file_name).to_str().unwrap(),
                        &format!("{NDK_IN}/*"),
                        "-d", src_dir.to_str().unwrap(),
                    ]
                ).unwrap();
                shell(
                    &cwd,
                    "cp",
                    &[
                        "--force",
                        "--recursive",
                        "--preserve",
                        src_dir.join(NDK_IN).to_str().unwrap(),
                        ndk_out_path.parent().unwrap().to_str().unwrap(),
                    ]
                ).unwrap();
            } else {
                // Extract only the bare minimum NDK to build makepad and an app's Rust-only contents.
                let mut ndk_extract = Vec::new();
                #[allow(non_snake_case)]
                for target in targets{
                    let sys_dir = target.sys_dir();
                    let clang = target.clang();
                    let unwind_dir = target.unwind_dir();
                    let SYS_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let SYS_OUT = &format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/{sys_dir}/33");
                    let UNWIND_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/linux-x86_64/lib64/clang/14.0.7/lib/linux/{unwind_dir}"); 
                    ndk_extract.extend_from_slice(&[
                        (copy_map(NDK_IN, NDK_OUT, &format!("bin/{clang}33-clang")), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/clang"), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/clang-14"), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/ld"), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/ld.lld"), true),
                        (copy_map(NDK_IN, NDK_OUT, "bin/lld"), true),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libxml2.so.2.9.13"), false),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libxml2.so"), false),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libc++.so"), false),
                        (copy_map(NDK_IN, NDK_OUT, "lib64/libc++.so.1"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtbegin_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "crtend_so.o"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libc.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libGLESv2.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libm.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "liblog.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libEGL.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libdl.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libaaudio.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libandroid.so"), false),                    
                        (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                        (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                        (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                        (format!("{UNWIND_IN}/libunwind.a|{SYS_OUT}/libunwind.a"), false),
                    ]);
                }
                let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
                unzip(4, src_dir, sdk_dir, URL_NDK_33_LINUX, &ndk_extract) ?;
            }
            
            const JDK_IN: &str = "jdk-17.0.2";
            const JDK_OUT: &str = "openjdk";
            untar(5, src_dir, sdk_dir, URL_OPENJDK_17_0_2_LINUX_X64, &[
                (&copy_map(JDK_IN, JDK_OUT, "bin/java"), true),
                (&copy_map(JDK_IN, JDK_OUT, "bin/jar"), true),
                (&copy_map(JDK_IN, JDK_OUT, "bin/javac"), true),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjli.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/jvm.cfg"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/server/libjsig.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/server/libjvm.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/modules"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/tzdb.dat"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjava.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libjimage.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libnet.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libnio.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libverify.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "lib/libzip.so"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/java.security"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/unlimited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_local.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/default_US_export.policy"), false),
                (&copy_map(JDK_IN, JDK_OUT, "conf/security/policy/limited/exempt_local.policy"), false),
            ]) ?;
        }
        HostOs::Unsupported => panic!()
    }
    Ok(())
}

