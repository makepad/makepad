
use makepad_miniz::zip_file::*;
use std::{
    path::{Path},
    fs::File,
    io::Write,
};

use crate::{
    android::*,
    shell::*
};

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
    shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    for target in targets{
        let toolchain = target.toolchain();
        println!("Installing rust nightly for {}", toolchain);
        shell_env_cap(&[],&std::env::current_dir().unwrap(), "rustup", &[
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
        println!("Downloading {step}/5: {}", url);
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

pub fn expand_sdk(sdk_dir: &Path, host_os: HostOs, _args: &[String], targets:&[AndroidTarget]) -> Result<(), String> {
    
    let src_dir = &sdk_dir.join("sources");
    
    fn unzip(step: usize, src_dir: &Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("Unzipping {step}/5: {}", url_file_name);
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
        println!("Untarring {step}/5: {}", url_file_name);
        shell(src_dir, "tar", &["-xf", src_dir.join(url_file_name).to_str().unwrap()]) ?;
        
        for (file_path, exec) in files {
            let (source_path, dest_path) = if let Some((a, b)) = file_path.split_once('|') {(a, b)}else {(*file_path, *file_path)};
            cp(&src_dir.join(source_path), &sdk_dir.join(dest_path), *exec) ?;
        }
        Ok(())
    }
    
    fn dmg_extract(step: usize, src_dir: &Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("Mounting and extracting dmg {step}/5: {}", url_file_name);
        
        let mount_point = &src_dir.join(&format!("mount_{url_file_name}"));
        mkdir(mount_point) ?;
        
        shell(src_dir, "hdiutil", &["attach", "-quiet", "-mountpoint", mount_point.to_str().unwrap(), src_dir.join(url_file_name).to_str().unwrap()]) ?;
        
        for (file_path, exec) in files {
            let (source_path, dest_path) = if let Some((a, b)) = file_path.split_once('|') {(a, b)}else {(*file_path, *file_path)};
            cp(&mount_point.join(source_path), &sdk_dir.join(dest_path), *exec) ?;
        }
        shell(sdk_dir, "umount", &[mount_point.to_str().unwrap()]) ?;
        Ok(())
    }
    
    fn copy_map(base_in: &str, base_out: &str, file: &str) -> String {
        format!("{base_in}/{file}|{base_out}/{file}")
    }
    
    match host_os {
        HostOs::WindowsX64 => {
            unzip(1, src_dir, sdk_dir, URL_PLATFORM_33, &[
                ("android-33-ext4/android.jar", false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_WINDOWS, &[
                ("android-13/aapt.exe", false),
                ("android-13/zipalign.exe", false),
                ("android-13/lib/apksigner.jar", false),
                ("android-13/lib/d8.jar", false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_WINDOWS, &[
                ("platform-tools/adb.exe", false),
                ("platform-tools/AdbWinApi.dll", false),
                ("platform-tools/AdbWinUsbApi.dll", false),
            ]) ?;
            const NDK_IN: &str = "android-ndk-r25c/toolchains/llvm/prebuilt/windows-x86_64";
            const NDK_OUT: &str = "NDK/toolchains/llvm/prebuilt/windows-x86_64";
            
            let mut ndk_extract = Vec::new();
            #[allow(non_snake_case)]
            for target in targets{
                let sys_dir = target.sys_dir();
                let clang = target.clang();
                let SYS_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/{sys_dir}/33");
                let SYS_OUT = &format!("NDK/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib/{sys_dir}/33");
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
                    (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libunwind.so"), false),
                ]);
            }
            let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
            unzip(4, src_dir, sdk_dir, URL_NDK_33_WINDOWS, &ndk_extract) ?;
            
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
                ("android-33-ext4/android.jar", false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_MACOS, &[
                ("android-13/aapt", true),
                ("android-13/zipalign", true),
                ("android-13/lib/apksigner.jar", false),
                ("android-13/lib/d8.jar", false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_MACOS, &[
                ("platform-tools/adb", true),
            ]) ?;
            const NDK_IN: &str = "AndroidNDK9519653.app/Contents/NDK/toolchains/llvm/prebuilt/darwin-x86_64";
            const NDK_OUT: &str = "NDK/toolchains/llvm/prebuilt/darwin-x86_64";
            
            let mut ndk_extract = Vec::new();
            #[allow(non_snake_case)]
            for target in targets{
                let sys_dir = target.sys_dir();
                let clang = target.clang();
                let SYS_IN = &format!("AndroidNDK9519653.app/Contents/NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/{sys_dir}/33");
                let SYS_OUT = &format!("NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/{sys_dir}/33");
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
                    (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libunwind.so"), false),
                ]);
            }
            let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
            dmg_extract(4, src_dir, sdk_dir, URL_NDK_33_MACOS, &ndk_extract) ?;
            
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
                ("android-33-ext4/android.jar", false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_LINUX, &[
                ("android-13/aapt", true),
                ("android-13/lib64/libc++.so", true),
                ("android-13/zipalign", true),
                ("android-13/lib/apksigner.jar", false),
                ("android-13/lib/d8.jar", false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_LINUX, &[
                ("platform-tools/adb", true),
            ]) ?;
            const NDK_IN: &str = "android-ndk-r25c/toolchains/llvm/prebuilt/linux-x86_64";
            const NDK_OUT: &str = "NDK/toolchains/llvm/prebuilt/linux-x86_64";
            
            let mut ndk_extract = Vec::new();
            #[allow(non_snake_case)]
            for target in targets{
                let sys_dir = target.sys_dir();
                let clang = target.clang();
                let SYS_IN = &format!("android-ndk-r25c/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/{sys_dir}/33");
                let SYS_OUT = &format!("NDK/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/{sys_dir}/33");
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
                    (copy_map(SYS_IN, SYS_OUT, "libamidi.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libcamera2ndk.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libnativewindow.so"), false),
                    (copy_map(SYS_IN, SYS_OUT, "libmediandk.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libgcc.so"), false),
                    (format!("{SYS_IN}/libc.so|{SYS_OUT}/libunwind.so"), false),
                ]);
            }
            let ndk_extract: Vec<(&str,bool)> = ndk_extract.iter().map(|s| (s.0.as_str(),s.1)).collect();
            unzip(4, src_dir, sdk_dir, URL_NDK_33_LINUX, &ndk_extract) ?;
            
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

