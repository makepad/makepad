
use makepad_miniz::zip_file::*;
use std::{
    path::{Path},
    fs::File,
    io::Write,
    fs,
};

use crate::{
    android::*,
    android::shell::*
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

fn url_file_name(url: &str) -> &str {
    url.rsplit_once("/").unwrap().1
}

pub fn download_sdk(sdk_dir: &Path, host_os: HostOs, _args: &[String]) -> Result<(), String> {
    // get current working directory
    let src_dir = &sdk_dir.join("sources");
    mkdir(src_dir) ?;
    
    fn curl(step: usize, src_dir: &Path, url: &str) -> Result<(), String> {
        //let https = HttpsConnection::connect("https://makepad.dev","https");
        println!("---- Downloading {step}/5: {} ----", url);
        shell(&src_dir, "curl", &[url, "--output", src_dir.join(url_file_name(url)).to_str().unwrap()]) ?;
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
                curl(5, &src_dir, URL_OPENJDK_17_0_2_MACOS_AARCH64) ?;
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
    println!("All Android SDK files downloaded in {:?}\nAs the next step, run: cargo makepad android expand-full-sdk", sdk_dir);
    // alright lets parse the sdk_path option
    Ok(())
}

pub fn expand_sdk(sdk_dir: &Path, host_os: HostOs, _args: &[String]) -> Result<(), String> {
    
    let src_dir = &sdk_dir.join("sources");
    
    fn unzip(step: usize, src_dir:&Path, sdk_dir: &Path, url: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Unzipping {step}/5: {} ----", url_file_name);
        let mut zip_file = File::open(src_dir.join(url_file_name))
            .map_err( | _ | format!("Cant open file {url_file_name}")) ?;
        
        // Hahahah i parsed my own zipfile. That was easier than using someones bad abstraction.
        let directory = zip_read_central_directory(&mut zip_file)
            .map_err( | e | format!("Can't read zipfile {url_file_name} {:?}", e)) ?;

        fn extract_file(directory: &ZipCentralDirectory, zip_file: &mut File, file_name: &str, output_file: &Path, exec:bool) -> Result<(), String> {
            if let Some(file_header) = directory.file_headers.iter().find( | v | v.file_name == file_name) {
                let data = file_header.extract(zip_file).map_err( | e | {
                    format!("Can't extract file from {file_name} {:?}", e)
                }) ?;

                mkdir(output_file.parent().unwrap()) ?;

                let mut output = File::create(output_file)
                    .map_err( | _ | format!("Cant open output file {:?}",output_file)) ?;

                output.write(&data)
                    .map_err( | _ | format!("Cant write output file {:?}",output_file)) ?;

                if exec{
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(output_file, PermissionsExt::from_mode(0o744))
                        .map_err( | _ | format!("Cant set exec permissions {:?}",output_file)) ?;
                }
                
                Ok(())
            }
            else {
                
                return Err(format!("File not found in zip {}", file_name))
            }
        }
        // ok so
        for (file_name, exec) in files {
            extract_file(&directory, &mut zip_file, file_name, &sdk_dir.join(file_name), *exec)?;
        }
        Ok(())
    }

    
    fn untar(step: usize, src_dir:&Path, sdk_dir: &Path, url: &str, src_base: &str, dst_base:&str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Untarring {step}/5: {} ----", url_file_name);
        shell(&src_dir, "tar", &["-xf", src_dir.join(url_file_name).to_str().unwrap()]) ?;

        for (file_path, exec) in files{
            cp(&src_dir.join(src_base).join(file_path), &sdk_dir.join(dst_base).join(file_path), *exec)?;
        } 
        Ok(())
    }
    
    fn dmg_extract(step: usize, src_dir:&Path, sdk_dir: &Path, url: &str, src_base: &str, files: &[(&str, bool)]) -> Result<(), String> {
        let url_file_name = url_file_name(url);
        println!("---- Mounting and extracting dmg {step}/5: {} ----", url_file_name);
        
        let mount_point = &src_dir.join(&format!("mount_{url_file_name}"));
        mkdir(mount_point) ?;
        
        shell(&src_dir, "hdiutil", &["attach", "-quiet", "-mountpoint", mount_point.to_str().unwrap(), src_dir.join(url_file_name).to_str().unwrap()]) ?;

        for (file_path, exec) in files{
            let (file_path, output_path) = if let Some((a,b)) = file_path.split_once("|"){(a,b)}else{(*file_path,*file_path)};
            
            cp(&mount_point.join(src_base).join(file_path), &sdk_dir.join(output_path), *exec)?;
        }
        shell(&sdk_dir, "umount", &[mount_point.to_str().unwrap()])?;
        Ok(())
    }
    
    match host_os {
        HostOs::WindowsX64 => {
        }
        HostOs::MacosX64 | HostOs::MacosAarch64 => {
            unzip(1, src_dir, sdk_dir, URL_PLATFORM_33, &[
                ("android-33-ext4/android.jar", false),
            ]) ?;
            unzip(2, src_dir, sdk_dir, URL_BUILD_TOOLS_33_MACOS, &[
                ("android-13/aapt", true),
                ("android-13/apksigner", true),
                ("android-13/zipalign", true),
                ("android-13/d8", true),
                ("android-13/lib/apksigner.jar", false),
                ("android-13/lib/d8.jar", false),
            ]) ?;
            unzip(3, src_dir, sdk_dir, URL_PLATFORM_TOOLS_33_MACOS, &[
                ("platform-tools/adb", true),
            ]) ?;
            const NDK_BASE: &'static str = "NDK/toolchains/llvm/prebuilt/darwin-x86_64";
            const SYSLIB: &'static str = "NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33";
            dmg_extract(4, src_dir, sdk_dir, URL_NDK_33_MACOS, "AndroidNDK9519653.app/Contents/", &[
                (&format!("{NDK_BASE}/bin/aarch64-linux-android33-clang"),true),
                (&format!("{NDK_BASE}/bin/clang"),true),
                (&format!("{NDK_BASE}/bin/ld"),true),
                (&format!("{NDK_BASE}/lib64/libxml2.2.9.13.dylib"),false),
                (&format!("{NDK_BASE}/lib64/libxml2.dylib"),false),
                (&format!("{NDK_BASE}/lib64/libxml2.2.9.13.dylib"),false),
                (&format!("{SYSLIB}/crtbegin_so.o"),false),
                (&format!("{SYSLIB}/crtend_so.o"),false),
                (&format!("{SYSLIB}/libc.so"),false),
                (&format!("{SYSLIB}/libGLESv2.so"),false),
                (&format!("{SYSLIB}/libm.so"),false),
                (&format!("{SYSLIB}/liblog.so"),false),
                (&format!("{SYSLIB}/libEGL.so"),false),
                (&format!("{SYSLIB}/libdl.so"),false),
                (&format!("{SYSLIB}/libaaudio.so"),false),
                (&format!("{SYSLIB}/libamidi.so"),false),
                (&format!("{SYSLIB}/libcamera2ndk.so"),false),
                (&format!("{SYSLIB}/libnativewindow.so"),false),
                (&format!("{SYSLIB}/libmediandk.so"),false),
                (&format!("{SYSLIB}/libc.so|{SYSLIB}/libgcc.so"),false),
                (&format!("{SYSLIB}/libc.so|{SYSLIB}/libunwind.so"),false),
            ])?;
            const JDK_EXTR: &'static str = "jdk-17.0.2.jdk/Contents/Home";
            untar(5, src_dir, sdk_dir, if host_os == HostOs::MacosX64 {URL_OPENJDK_17_0_2_MACOS_X64}else {URL_OPENJDK_17_0_2_MACOS_AARCH64}, JDK_EXTR, "openjdk", &[
                ("bin/java", true),
                ("bin/jar", true),
                ("bin/javac", true),
                ("lib/libjli.dylib", false),
                ("lib/jvm.cfg", false),
                ("lib/server/libjsig.dylib", false),
                ("lib/server/libjvm.dylib", false),
                ("lib/modules", false),
                ("lib/tzdb.dat", false),
                ("lib/libjava.dylib", false),
                ("lib/libjimage.dylib", false),
                ("lib/libnet.dylib", false),
                ("lib/libnio.dylib", false),
                ("lib/libverify.dylib", false),
                ("lib/libzip.dylib", false),
                ("conf/security/java.policy", false),
                ("conf/security/java.security", false),
                ("conf/security/policy/unlimited/default_local.policy", false),
                ("conf/security/policy/unlimited/default_US_export.policy", false),
                ("conf/security/policy/limited/default_local.policy", false),
                ("conf/security/policy/limited/default_US_export.policy", false),
                ("conf/security/policy/limited/exempt_local.policy", false),
            ])?;
        }
        HostOs::LinuxX64 => {
        }
        HostOs::Unsupported => panic!()
    }
    Ok(())
}

