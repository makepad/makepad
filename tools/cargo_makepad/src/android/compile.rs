use std::path::{Path, PathBuf};
use crate::android::{HostOs, AndroidTarget};
use crate::utils::*;
use crate::makepad_shell::*;
use super::sdk::{NDK_VERSION_FULL, BUILD_TOOLS_DIR, SDK_VERSION, PLATFORMS_DIR, API_LEVEL};

fn aapt_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(SDK_VERSION).join("aapt")
}

fn d8_jar_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(SDK_VERSION).join("lib/d8.jar")
}

fn apksigner_jar_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(SDK_VERSION).join("lib/apksigner.jar")
}

fn zipalign_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(SDK_VERSION).join("zipalign")
}

fn android_jar_path(sdk_dir: &Path) -> PathBuf {
    sdk_dir.join(PLATFORMS_DIR).join(API_LEVEL).join("android.jar")
}


#[derive(Debug)]
struct BuildPaths {
    tmp_dir: PathBuf,
    out_dir: PathBuf,
    manifest_file: PathBuf,
    java_file: PathBuf,
    java_class: PathBuf,
    dst_unaligned_apk: PathBuf,
    dst_apk: PathBuf,
}

pub struct BuildResult {
    dst_apk: PathBuf,
    java_url: String,
}

fn manifest_xml(label:&str, class_name:&str, url:&str)->String{
    format!(r#"<?xml version="1.0" encoding="utf-8"?>
    <manifest xmlns:android="http://schemas.android.com/apk/res/android"
        xmlns:tools="http://schemas.android.com/tools"
        package="{url}">
        <application
            android:label="{label}"
            android:theme="@android:style/Theme.NoTitleBar.Fullscreen"
            android:allowBackup="true"
            android:supportsRtl="true"
            android:debuggable="true"
            android:largeHeap="true"
            tools:targetApi="33">
            <meta-data android:name="android.max_aspect" android:value="2.1" />
            <activity
                android:name=".{class_name}"
                android:configChanges="orientation|screenSize|keyboardHidden"
                android:exported="true">
                <intent-filter>
                    <action android:name="android.intent.action.MAIN" />
                    <category android:name="android.intent.category.LAUNCHER" />
                </intent-filter>
            </activity>
        </application>
        <uses-sdk android:targetSdkVersion="33"/>
        <uses-feature android:glEsVersion="0x00020000" android:required="true"/>
        <uses-feature android:name="android.hardware.bluetooth_le" android:required="true"/>
        <uses-feature android:name="android.software.midi" android:required="true"/>
        <uses-permission android:name="android.permission.READ_EXTERNAL_STORAGE" />
        <uses-permission android:name="android.permission.READ_MEDIA_VIDEO"  />
        <uses-permission android:name="android.permission.READ_MEDIA_IMAGES"  />
        <uses-permission android:name="android.permission.INTERNET" />
        <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
        <uses-permission android:name="android.permission.BLUETOOTH"/>
        <uses-permission android:name="android.permission.BLUETOOTH_CONNECT"/>
        <uses-permission android:name="android.permission.CAMERA"/>
        <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"/>
        <uses-permission android:name="android.permission.USE_BIOMETRIC" />
    </manifest>
    "#)
}

fn main_java(url:&str)->String{
    format!(r#"
        package {url};
        import dev.makepad.android.MakepadActivity;
        public class MakepadApp extends MakepadActivity{{
        }}
    "#)
}

fn rust_build(sdk_dir: &Path, host_os: HostOs, args: &[String], android_targets:&[AndroidTarget]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();

    for android_target in android_targets {
        let clang_filename = format!("{}33-clang", android_target.clang());
        
        let bin_path = |bin_filename: &str, windows_extension: &str| match host_os {
            HostOs::MacosX64     => format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/darwin-x86_64/bin/{bin_filename}"),
            HostOs::MacosAarch64 => format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/darwin-x86_64/bin/{bin_filename}"),
            HostOs::WindowsX64   => format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/windows-x86_64/bin/{bin_filename}.{windows_extension}"),
            HostOs::LinuxX64     => format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/linux-x86_64/bin/{bin_filename}"),
            _ => panic!()
        };
        let full_clang_path       = sdk_dir.join(bin_path(&clang_filename, "cmd"));
        let full_llvm_ar_path     = sdk_dir.join(bin_path("llvm-ar", "exe"));
        let full_llvm_ranlib_path = sdk_dir.join(bin_path("llvm-ranlib", "exe"));

        let toolchain = android_target.toolchain();
        let target_opt = format!("--target={toolchain}");

        let base_args = &[
            "run",
            "nightly",
            "cargo",
            "rustc",
            "--lib",
            "--crate-type=cdylib",
            &target_opt
        ]; 
        let mut args_out = Vec::new();
        args_out.extend_from_slice(base_args);
        for arg in args {
            args_out.push(arg);
        }

        let target_arch_str = android_target.to_str();
        let cfg_flag = format!("--cfg android_target=\"{}\"", target_arch_str);
         
        shell_env(
            &[
                // Set the linker env var to the path of the target-specific `clang` binary.
                (&android_target.linker_env_var(), full_clang_path.to_str().unwrap()),

                // We set standard Android-related env vars to allow other crates
                ("ANDROID_HOME",          sdk_dir.to_str().unwrap()),
                ("ANDROID_SDK_ROOT",      sdk_dir.to_str().unwrap()),
                ("ANDROID_SDK_VERSION",   super::sdk::SDK_VERSION),
                ("ANDROID_API_LEVEL",     super::sdk::API_LEVEL),
                ("JAVA_HOME",             sdk_dir.join("openjdk").to_str().unwrap()),

                // We set these three env vars to allow native library C/C++ builds to succeed with no additional app-side config.
                // The naming conventions of these env variable keys are established by the `cc` Rust crate.
                (&format!("CC_{toolchain}"),     full_clang_path.to_str().unwrap()),
                (&format!("AR_{toolchain}"),     full_llvm_ar_path.to_str().unwrap()),
                (&format!("RANLIB_{toolchain}"), full_llvm_ranlib_path.to_str().unwrap()),

                ("RUSTFLAGS", &cfg_flag),
                ("MAKEPAD", "lines"),
            ],
            &cwd,
            "rustup",
            &args_out
        ) ?;
    }

    Ok(())
}

fn prepare_build(underscore_build_crate: &str, java_url: &str, app_label: &str) -> Result<BuildPaths, String> {
    let cwd = std::env::current_dir().unwrap();

    let tmp_dir = cwd.join(format!("target/makepad-android-apk/{underscore_build_crate}/tmp"));
    let out_dir = cwd.join(format!("target/makepad-android-apk/{underscore_build_crate}/apk"));
    
    // lets remove tmp and out dir
    let _ = rmdir(&tmp_dir);
    let _ = rmdir(&out_dir);
    mkdir(&tmp_dir) ?; 
    mkdir(&out_dir) ?;

    let manifest_xml = manifest_xml(app_label, "MakepadApp", java_url);
    let manifest_file = tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;
    
    let main_java = main_java(java_url);
    let java_path = java_url.replace('.',"/");
    let java_file = tmp_dir.join(&java_path).join("MakepadApp.java");
    let java_class = out_dir.join(&java_path).join("MakepadApp.class");
    write_text(&java_file, &main_java)?;

    let apk_filename = to_snakecase(app_label);
    let dst_unaligned_apk = out_dir.join(format!("{apk_filename}.unaligned.apk"));
    let dst_apk = out_dir.join(format!("{apk_filename}.apk"));

    let _ = rm(&dst_unaligned_apk);
    let _ = rm(&dst_apk);

    Ok(BuildPaths{
        tmp_dir,
        out_dir,
        manifest_file,
        java_file,
        java_class,
        dst_unaligned_apk,
        dst_apk,
    })
}

fn build_r_class(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
       &cwd,
       &aapt_path(sdk_dir).to_str().unwrap(),
       &[
           "package",
           "-f",
           "-m",
           "-I",
           (android_jar_path(sdk_dir).to_str().unwrap()),
           "-S",
           (cargo_manifest_dir.join("src/android/res").to_str().unwrap()),
           "-M",
           (build_paths.manifest_file.to_str().unwrap()),
           "-J",
           (build_paths.tmp_dir.to_str().unwrap()),
           "--custom-package",
           "dev.makepad.android",
           (build_paths.out_dir.to_str().unwrap()),
       ]
    ) ?;

    Ok(())
}

fn compile_java(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    let makepad_package_path = "dev/makepad/android";
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let r_class_path = build_paths.tmp_dir.join(makepad_package_path).join("R.java");
    let makepad_java_classes_dir = &cargo_manifest_dir.join("src/android/java/").join(makepad_package_path);

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/javac").to_str().unwrap(),
        &[
            "-classpath", 
            (android_jar_path(sdk_dir).to_str().unwrap()),
            "-Xlint:deprecation",
            "-d", 
            (build_paths.out_dir.to_str().unwrap()),
            (r_class_path.to_str().unwrap()),
            (makepad_java_classes_dir.join("MakepadNative.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("MakepadActivity.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("MakepadNetwork.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("MakepadWebSocket.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("MakepadWebSocketReader.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("ByteArrayMediaDataSource.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("VideoPlayer.java").to_str().unwrap()),
            (makepad_java_classes_dir.join("VideoPlayerRunnable.java").to_str().unwrap()),
            (build_paths.java_file.to_str().unwrap())
        ]   
    ) ?; 

    Ok(())
}

fn build_dex(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    let makepad_package_path = "dev/makepad/android";
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let compiled_java_classes_dir = build_paths.out_dir.join(makepad_package_path);

    shell_env_cap( 
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &[ 
            "-cp",
            (d8_jar_path(sdk_dir).to_str().unwrap()),
            "com.android.tools.r8.D8",
            "--classpath",
            (android_jar_path(sdk_dir).to_str().unwrap()),
            "--output",  
            (build_paths.out_dir.to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadNative.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadActivity.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadSurface.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("ResizingLayout.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadNetwork.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadWebSocket.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadWebSocketReader.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("HttpResponse.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("ByteArrayMediaDataSource.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("VideoPlayer.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("VideoPlayerRunnable.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("VideoPlayer$1.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("VideoPlayer$2.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("VideoPlayer$3.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadActivity$1.class").to_str().unwrap()),
            (compiled_java_classes_dir.join("MakepadActivity$2.class").to_str().unwrap()),
            (build_paths.java_class.to_str().unwrap()),
        ]
    ) ?;

    Ok(())
} 

fn build_unaligned_apk(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
       &cwd,
       aapt_path(sdk_dir).to_str().unwrap(),
       &[ 
           "package",
           "-f",
           "-F",
           (build_paths.dst_unaligned_apk.to_str().unwrap()),
           "-I",
           (android_jar_path(sdk_dir).to_str().unwrap()),
           "-M",
           (build_paths.manifest_file.to_str().unwrap()),
           "-S",
           (cargo_manifest_dir.join("src/android/res").to_str().unwrap()),
           (build_paths.out_dir.to_str().unwrap()),
       ]
    ) ?;

    Ok(())
}

fn add_rust_library(sdk_dir: &Path, underscore_target: &str, build_paths: &BuildPaths, android_targets: &[AndroidTarget], args: &[String]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let profile = get_profile_from_args(args);
    
    for android_target in android_targets {
        let abi = android_target.abi_identifier();
        mkdir(&build_paths.out_dir.join(format!("lib/{abi}"))) ?;

        let android_target_dir = android_target.toolchain();
        let binary_path = format!("lib/{abi}/libmakepad.so");
        if profile == "debug"{
            println!("WARNING - compiling a DEBUG build of the application, this creates a very slow and big app. Try adding --release for a fast, or --profile=small for a small build.");
        }
        let src_lib = cwd.join(format!("target/{android_target_dir}/{profile}/lib{underscore_target}.so"));
        let dst_lib = build_paths.out_dir.join(binary_path.clone());
        cp(&src_lib, &dst_lib, false) ?;

        shell_env_cap(&[], &build_paths.out_dir, aapt_path(sdk_dir).to_str().unwrap(), &[
            "add",
            (build_paths.dst_unaligned_apk.to_str().unwrap()),
            &binary_path,
        ]) ?;
    }

    Ok(())
}

fn add_resources(sdk_dir: &Path, build_crate: &str, build_paths: &BuildPaths) -> Result<(), String> {
    let mut assets_to_add: Vec<String> = Vec::new();
    
    let build_crate_dir = get_crate_dir(build_crate) ?;
    let local_resources_path = build_crate_dir.join("resources");
    if local_resources_path.is_dir() {
        let underscore_build_crate = build_crate.replace('-', "_");
        let dst_dir = build_paths.out_dir.join(format!("assets/makepad/{underscore_build_crate}/resources"));
        mkdir(&dst_dir) ?;
        
        cp_all(&local_resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display().to_string().replace("\\","/");
            assets_to_add.push(format!("assets/makepad/{underscore_build_crate}/resources/{path}"));
        }
    }

    let resources = get_crate_resources(build_crate);
    for (name, resources_path) in resources.iter() {
        let dst_dir = build_paths.out_dir.join(format!("assets/makepad/{name}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display().to_string();
            assets_to_add.push(format!("assets/makepad/{name}/resources/{path}"));
        }
    }

    let mut aapt_args = vec![
        "add",
        build_paths.dst_unaligned_apk.to_str().unwrap(),
    ];
    for asset in &assets_to_add {  
        aapt_args.push(asset);
    }

    shell_env_cap(&[], &build_paths.out_dir, aapt_path(sdk_dir).to_str().unwrap(), &aapt_args) ?;

    Ok(())
}

fn build_zipaligned_apk(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    shell_env_cap(&[], &build_paths.out_dir, zipalign_path(sdk_dir).to_str().unwrap(), &[
       "-v",
       "-f",
       "4",
       (build_paths.dst_unaligned_apk.to_str().unwrap()),
       (build_paths.dst_apk.to_str().unwrap()),
    ]) ?;

    Ok(())
}

fn sign_apk(sdk_dir: &Path, build_paths: &BuildPaths) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");

    shell_env_cap(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            (apksigner_jar_path(sdk_dir).to_str().unwrap()),
            "sign",
            "-v",
            "-ks",
            (cargo_manifest_dir.join("debug.keystore").to_str().unwrap()),
            "--ks-key-alias",
            "androiddebugkey",
            "--ks-pass",
            "pass:android",
            (build_paths.dst_apk.to_str().unwrap())
        ]
    ) ?;

    Ok(())
}

pub fn build(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String], android_targets:&[AndroidTarget]) -> Result<BuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");

    let java_url = package_name.unwrap_or_else(|| format!("dev.makepad.{underscore_build_crate}"));
    let app_label = app_label.unwrap_or_else(|| format!("{underscore_build_crate}"));

    rust_build(sdk_dir, host_os, args, android_targets)?;
    let build_paths = prepare_build(build_crate, &java_url, &app_label)?;

    println!("Compiling APK & R.java files");
    build_r_class(sdk_dir, &build_paths)?;
    compile_java(sdk_dir, &build_paths)?;

    println!("Building APK");
    build_dex(sdk_dir, &build_paths)?;
    build_unaligned_apk(sdk_dir, &build_paths)?;
    add_rust_library(sdk_dir, &underscore_build_crate, &build_paths, android_targets, args)?;
    add_resources(sdk_dir, build_crate, &build_paths)?;
    build_zipaligned_apk(sdk_dir, &build_paths)?;
    sign_apk(sdk_dir, &build_paths)?;

    println!("Compile APK completed");
    Ok(BuildResult{
        dst_apk: build_paths.dst_apk,
        java_url,
    })
}

pub fn run(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String], targets:&[AndroidTarget]) -> Result<(), String> {
    let result = build(sdk_dir, host_os, package_name, app_label, args, targets)?;
    
    let cwd = std::env::current_dir().unwrap();
    //println!("Installing android application");
    shell_env_cap(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "install",
        "-r",
        (result.dst_apk.to_str().unwrap()),
    ]) ?;
    println!("Starting android application: {}", result.dst_apk.file_name().unwrap().to_str().unwrap());
    shell_env_cap(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "shell",
        "am",
        "start",
        "-n",
        &format!("{0}/{0}.MakepadApp", result.java_url)
    ]) ?;  
    #[allow(unused_assignments)]
    let mut pid = None;
    loop{
        if let Ok(thing) = shell_env_cap(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
            "shell", 
            "pidof", 
            &result.java_url,
        ]){
            pid = Some(thing.trim().to_string());
            break;
        }
    }
    shell_env(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "logcat",
        "--pid",
        &pid.unwrap(),
        "Makepad:D *:S"
    ]) ?;
    Ok(())
}

pub fn adb(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    shell_env(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &args_out)?;
    Ok(())
}

pub fn java(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &args_out
    ) ?;
    Ok(())
}

pub fn javac(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<(), String> {
    let mut args_out = Vec::new();
    for arg in args {
        args_out.push(arg.as_ref());
    }
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/javac").to_str().unwrap(),
        &args_out
    ) ?;
    Ok(())
}



fn to_snakecase(label: &str) -> String {
    let mut snakecase = String::new();
    let mut previous_was_underscore = false;

    for c in label.chars() {
        if c.is_whitespace() {
            previous_was_underscore = true;
        } else if c.is_uppercase() {
            if !previous_was_underscore && !snakecase.is_empty() {
                snakecase.push('_');
            }
            snakecase.extend(c.to_lowercase());
            previous_was_underscore = false;
        } else {
            snakecase.push(c);
            previous_was_underscore = false;
        }
    }
    snakecase
}
