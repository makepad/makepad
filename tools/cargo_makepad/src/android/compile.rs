use std::{
    path::{Path, PathBuf},
    collections::HashSet,
};
use crate::android::HostOs;
use crate::shell::*;

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
            tools:targetApi="33">
            <meta-data android:name="android.max_aspect" android:value="2.1" />
            <activity
                android:name="{class_name}"
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
        <uses-permission android:name="android.permission.INTERNET" />
        <uses-permission android:name="android.permission.ACCESS_NETWORK_STATE" />
        <uses-permission android:name="android.permission.BLUETOOTH"/>
        <uses-permission android:name="android.permission.BLUETOOTH_CONNECT"/>
        <uses-permission android:name="android.permission.CAMERA"/>
        <uses-permission android:name="android.permission.ACCESS_FINE_LOCATION"/>
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

fn get_target_from_args(args: &[String]) -> Result<&str, String> {
    if args.len()<1 {
        return Err("Not enough arguments to build".into());
    }
    if args[0] == "-p" {
        if args.len()<2 { 
            return Err("Not enough arguments to build".into());
        }
        return Ok(&args[1])
    }
    else {
        return Ok(&args[0])
    };
}

fn rust_build(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    
    let linker = match host_os{
        HostOs::MacosX64=>"NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang",
        HostOs::MacosAarch64=>"NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang",
        HostOs::WindowsX64=>"NDK/toolchains/llvm/prebuilt/windows-x86_64/bin/aarch64-linux-android33-clang.cmd",
        HostOs::LinuxX64=>"NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android33-clang",
        _=>panic!()
    };

    let base_args = &[
        "run",
        "nightly",
        "cargo",
        "rustc",
        "--lib",
        "--crate-type=cdylib",
        "--release",
        "--target=aarch64-linux-android"
    ]; 
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(&arg);
    }
    
    shell_env(
        &[
            ("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", &sdk_dir.join(linker).to_str().unwrap()),
            ("MAKEPAD", "lines"),
        ],
        &cwd,
        "rustup",
        &args_out
    ) ?;

    Ok(())
}

struct BuildPaths{
    tmp_dir: PathBuf,
    out_dir: PathBuf,
}

struct JavaRootFiles{
    manifest_file: PathBuf,
    java_file: PathBuf,
    java_class: PathBuf,
}

pub struct BuildResult{
    dst_apk: PathBuf,
    java_url: String,
}

fn prepare_build_dirs(underscore_target: &str) -> Result<BuildPaths, String> {
    let cwd = std::env::current_dir().unwrap();

    let tmp_dir = cwd.join(format!("target/aarch64-linux-android-apk/{underscore_target}/tmp"));
    let out_dir = cwd.join(format!("target/aarch64-linux-android-apk/{underscore_target}/apk"));
    
    // lets remove tmp and out dir
    let _ = rmdir(&tmp_dir);
    let _ = rmdir(&out_dir);
    mkdir(&tmp_dir) ?; 
    mkdir(&out_dir) ?;

    Ok(BuildPaths{
        tmp_dir,
        out_dir,
    })
}

fn generate_root_java_files(build_paths: &BuildPaths, java_url: &str, app_label: &str) -> Result<JavaRootFiles, String> {
    // alright lets go and generate the root java file
    // and the manifest xml
    // then build the dst_apk
    // we'll leave the classname as is
    let manifest_xml = manifest_xml(&app_label, "MakepadApp", &java_url);
    let manifest_file = build_paths.tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;
    
    let main_java = main_java(&java_url);
    let java_path = java_url.replace(".","/");
    let java_file = build_paths.tmp_dir.join(&java_path).join("MakepadApp.java");
    let java_class = build_paths.out_dir.join(&java_path).join("MakepadApp.class");
    write_text(&java_file, &main_java)?;

    Ok(JavaRootFiles{
        manifest_file,
        java_file,
        java_class,
    })
}

fn build_r_class(sdk_dir: &Path, build_paths: &BuildPaths, manifest_file: &PathBuf) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
       &cwd,
       &sdk_dir.join("android-13/aapt").to_str().unwrap(),
       &[
           "package",
           "-f",
           "-m",
           "-I",
           &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
           "-S",
           &cargo_manifest_dir.join("src/android/res").to_str().unwrap(),
           "-M",
           &manifest_file.to_str().unwrap(),
           "-J",
           &build_paths.tmp_dir.to_str().unwrap(),
           "--custom-package",
           "dev.makepad.android",
           &build_paths.out_dir.to_str().unwrap(),
       ]
    ) ?;

    Ok(())
}

fn compile_java(sdk_dir: &Path, build_paths: &BuildPaths, java_file: &PathBuf) -> Result<(), String> {
    let makepad_package_path = "dev/makepad/android";
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let r_class_path = build_paths.tmp_dir.join(&makepad_package_path).join("R.java");
    let makepad_java_classes_dir = &cargo_manifest_dir.join("src/android/java/").join(&makepad_package_path);

    shell_env(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/javac").to_str().unwrap(),
        &[
            "-classpath", 
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "-Xlint:deprecation",
            "-d", 
            &build_paths.out_dir.to_str().unwrap(),
            &r_class_path.to_str().unwrap(),
            &makepad_java_classes_dir.join("Makepad.java").to_str().unwrap(),
            &makepad_java_classes_dir.join("MakepadActivity.java").to_str().unwrap(),
            &makepad_java_classes_dir.join("MakepadSurfaceView.java").to_str().unwrap(),
            &java_file.to_str().unwrap()
        ]   
    ) ?; 

    Ok(())
}

fn build_dex(sdk_dir: &Path, out_dir: &PathBuf, java_class: &PathBuf) -> Result<(), String> {
    let makepad_package_path = "dev/makepad/android";
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();

    let compiled_java_classes_dir = out_dir.join(&makepad_package_path);

    shell_env_cap( 
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/java").to_str().unwrap(),
        &[ 
            "-cp",
            &sdk_dir.join("android-13/lib/d8.jar").to_str().unwrap(),
            "com.android.tools.r8.D8",
            "--classpath",
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "--output",
            &out_dir.to_str().unwrap(),
            &compiled_java_classes_dir.join("Makepad.class").to_str().unwrap(),
            &compiled_java_classes_dir.join("MakepadActivity.class").to_str().unwrap(),
            &compiled_java_classes_dir.join("MakepadSurfaceView.class").to_str().unwrap(),
            &compiled_java_classes_dir.join("Makepad$Callback.class").to_str().unwrap(),
            &java_class.to_str().unwrap(),
        ]
    ) ?;

    Ok(())
}

fn build_unaligned_apk(sdk_dir: &Path, dst_unaligned_apk: &Path, out_dir: &PathBuf, manifest_file: &PathBuf) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
       &cwd,
       &sdk_dir.join("android-13/aapt").to_str().unwrap(),
       &[
           "package",
           "-f",
           "-F",
           &dst_unaligned_apk.to_str().unwrap(),
           "-I",
           &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
           "-M",
           &manifest_file.to_str().unwrap(),
           "-S",
           &cargo_manifest_dir.join("src/android/res").to_str().unwrap(),
           &out_dir.to_str().unwrap(),
       ]
    ) ?;

    Ok(())
}

fn add_rust_library(sdk_dir: &Path, dst_unaligned_apk: &Path, underscore_target: &str, out_dir: &PathBuf) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();

    mkdir(&out_dir.join("lib/arm64-v8a")) ?;
    
    let src_lib = cwd.join("target/aarch64-linux-android/release/").join(&format!("lib{underscore_target}.so"));
    let dst_lib = out_dir.join("lib/arm64-v8a").join("libmakepad.so");
    cp(&src_lib, &dst_lib, false) ?;

    shell_env_cap(&[], &out_dir, &sdk_dir.join("android-13/aapt").to_str().unwrap(), &[
        "add",
        &dst_unaligned_apk.to_str().unwrap(),
        "lib/arm64-v8a/libmakepad.so",
    ]) ?;

    Ok(())
}

fn add_resources(sdk_dir: &Path, dst_unaligned_apk: &Path, target: &str, out_dir: &PathBuf) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let mut assets_to_add: Vec<String> = Vec::new();

    let target_crate_dir = get_target_crate_dir(target) ?;
    let local_resources_path = target_crate_dir.join("resources");
    if local_resources_path.is_dir() {
        let underscore_target = target.replace("-", "_");
        let dst_dir = out_dir.join(format!("assets/makepad/{underscore_target}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(&local_resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display();
            assets_to_add.push(format!("assets/makepad/{underscore_target}/resources/{path}"));
        }
    }

    let mut dependencies = HashSet::new();
    if let Ok(cargo_tree_output) = shell_env_cap(&[], &cwd, "cargo", &["tree", "-p", target]) {
        for line in cargo_tree_output.lines().skip(1) {
            if let Some((name, path)) = extract_dependency_info(line) {
                let resources_path = Path::new(&path).join("resources");
                if resources_path.is_dir() {
                    dependencies.insert((name.replace('-',"_"), resources_path));
                }
            }
        }
    }

    for (name, resources_path) in dependencies.iter() {
        let dst_dir = out_dir.join(format!("assets/makepad/{name}/resources"));
        mkdir(&dst_dir) ?;
        cp_all(&resources_path, &dst_dir, false) ?;

        let assets = ls(&dst_dir) ?;
        for path in &assets {
            let path = path.display();
            assets_to_add.push(format!("assets/makepad/{name}/resources/{path}"));
        }
    }

    let mut aapt_args = vec![
        "add",
        &dst_unaligned_apk.to_str().unwrap(),
    ];
    for asset in &assets_to_add {
        aapt_args.push(asset);
    }

    shell_env_cap(&[], &out_dir, &sdk_dir.join("android-13/aapt").to_str().unwrap(), &aapt_args) ?;

    Ok(())
}

fn build_zipaligned_apk(sdk_dir: &Path, dst_unaligned_apk: &Path, dst_apk: &Path, out_dir: &PathBuf) -> Result<(), String> {
    shell_env_cap(&[], &out_dir, &sdk_dir.join("android-13/zipalign").to_str().unwrap(), &[
       "-v",
       "-f",
       "4",
       &dst_unaligned_apk.to_str().unwrap(),
       &dst_apk.to_str().unwrap(),
    ]) ?;

    Ok(())
}

fn sign_apk(sdk_dir: &Path, dst_apk: &Path) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");

    shell_env_cap(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            &sdk_dir.join("android-13/lib/apksigner.jar").to_str().unwrap(),
            "sign",
            "-v",
            "-ks",
            &cargo_manifest_dir.join("debug.keystore").to_str().unwrap(),
            "--ks-key-alias",
            "androiddebugkey",
            "--ks-pass",
            "pass:android",
            &dst_apk.to_str().unwrap() 
        ]
    ) ?;

    Ok(())
}

fn get_target_crate_dir(target: &str) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    if let Ok(output) = shell_env_cap(&[], &cwd, "cargo", &["pkgid", "-p", target]) {
        return Ok(output.trim_start_matches("file://").split('#').next().unwrap().into())
    } else {
        return Err(format!("Failed to get crate dir for: {}", target))
    }
}

pub fn build(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String]) -> Result<BuildResult, String> {
    let target = get_target_from_args(args)?;
    let underscore_target = target.replace("-", "_");

    let java_url = package_name.unwrap_or_else(|| format!("dev.makepad.{underscore_target}"));
    let app_label = app_label.unwrap_or_else(|| format!("{underscore_target}"));
    let apk_filename = to_snakecase(&app_label);

    rust_build(sdk_dir, host_os, args)?;
    let build_paths = prepare_build_dirs(&underscore_target)?;
    let root_java_files = generate_root_java_files(&build_paths, &java_url, &app_label)?;

    let dst_unaligned_apk = build_paths.out_dir.join(format!("{apk_filename}.unaligned.apk"));
    let dst_apk = build_paths.out_dir.join(format!("{apk_filename}.apk"));

    let _ = rm(&dst_unaligned_apk);
    let _ = rm(&dst_apk);

    println!("Compiling APK & R.java files");
    build_r_class(sdk_dir, &build_paths, &root_java_files.manifest_file)?;
    compile_java(sdk_dir, &build_paths, &root_java_files.java_file)?;

    println!("Building APK");
    build_dex(sdk_dir, &build_paths.out_dir, &root_java_files.java_class)?;
    build_unaligned_apk(sdk_dir, &dst_unaligned_apk, &build_paths.out_dir, &root_java_files.manifest_file)?;
    add_rust_library(sdk_dir, &dst_unaligned_apk, &underscore_target, &build_paths.out_dir)?;
    add_resources(sdk_dir, &dst_unaligned_apk, target, &build_paths.out_dir)?;
    build_zipaligned_apk(sdk_dir, &dst_unaligned_apk, &dst_apk, &build_paths.out_dir)?;
    sign_apk(sdk_dir, &dst_apk)?;

    println!("Compile APK completed");
    Ok(BuildResult{
        dst_apk,
        java_url,
    })
}

pub fn run(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String]) -> Result<(), String> {
    let result = build(sdk_dir, host_os, package_name, app_label, args)?;
    
    let cwd = std::env::current_dir().unwrap();
    //println!("Installing android application");
    shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "install",
        "-r",
        &result.dst_apk.to_str().unwrap(),
    ]) ?;
    println!("Starting android application: {}", result.dst_apk.file_name().unwrap().to_str().unwrap());
    shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "shell",
        "am",
        "start",
        "-n",
        &format!("{0}/{0}.MakepadApp", result.java_url)
    ]) ?;  
    #[allow(unused_assignments)]
    let mut pid = None;
    loop{
        if let Ok(thing) = shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
            "shell", 
            "pidof", 
            &result.java_url,
        ]){
            pid = Some(thing.trim().to_string());
            break;
        }
    }
    shell_env(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
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
    shell_env(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &args_out)?;
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
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/java").to_str().unwrap(),
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
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/javac").to_str().unwrap(),
        &args_out
    ) ?;
    Ok(())
}

fn extract_dependency_info(line: &str) -> Option<(String, String)> {
    let dependency_output_start = line.find(|c: char| c.is_alphanumeric())?;
    let dependency_output = &line[dependency_output_start..];

    let mut tokens = dependency_output.split(' ');
    if let Some(name) = tokens.next() {
        for token in tokens.collect::<Vec<&str>>() {
            if token == "(*)" || token == "(proc-macro)" {
                continue;
            }
            if token.starts_with("(") {
                let path = token[1..token.len() - 1].to_owned();
                return Some((name.to_string(), path))
            }
        }
    }
    None
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