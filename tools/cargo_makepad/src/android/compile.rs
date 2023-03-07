use std::{
    path::{Path, PathBuf},
};
use crate::android::HostOs;
use crate::android::shell::*;

pub fn build(sdk_dir: &Path, _host_os: HostOs, args: &[String]) -> Result<PathBuf, String> {
    if args.len()<1 {
        return Err("Not enough arguments to build".into());
    }
    let target = if args[0] == "-p" {
        if args.len()<2 {
            return Err("Not enough arguments to build".into());
        }
        &args[1]
    }
    else {
        &args[0]
    };
    
    // alright lets do the rust stuff.
    let cwd = std::env::current_dir().unwrap();
    let base_args = &[
        "+nightly",
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
        &[("CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER", &sdk_dir.join("NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang").to_str().unwrap())],
        &cwd,
        "cargo",
        &args_out
    ) ?;

    println!("Packaging android application");
    
    let cwd = std::env::current_dir().unwrap();
    let out_dir = cwd.join("target/aarch64-linux-android-apk").join(target);
    let src_apk = cwd.join("target/aarch64-linux-android-apk/makepad_base_apk").join("makepad_base_apk.apk");
    let underscore_target = target.replace("-", "_");
    let dst_apk = out_dir.join(format!("{underscore_target}.apk"));
    
    mkdir(&out_dir.join("lib/arm64-v8a")) ?;
    
    cp(&src_apk, &dst_apk, false) ?;
    
    let src_lib = cwd.join("target/aarch64-linux-android/release/").join(&format!("lib{underscore_target}.so"));
    let dst_lib = out_dir.join("lib/arm64-v8a").join("libmakepad.so");
    cp(&src_lib, &dst_lib, false) ?;

    shell_env_cap(&[], &out_dir, &sdk_dir.join("android-13/aapt").to_str().unwrap(), &[
        "add",
        &dst_apk.to_str().unwrap(),
        "lib/arm64-v8a/libmakepad.so",
    ]) ?;
    
    let font_dir = out_dir.join("assets/makepad/makepad_widgets/resources");
    mkdir(&font_dir) ?;
    cp(&cwd.join("widgets/resources/IBMPlexSans-Text.ttf"), &font_dir.join("IBMPlexSans-Text.ttf"), false) ?;
    cp(&cwd.join("widgets/resources/IBMPlexSans-SemiBold.ttf"), &font_dir.join("IBMPlexSans-SemiBold.ttf"), false) ?;
    cp(&cwd.join("widgets/resources/LiberationMono-Regular.ttf"), &font_dir.join("LiberationMono-Regular.ttf"), false) ?;

    cp(&cwd.join("examples/ironfish/resources/tinrs.png"), &out_dir.join("assets/makepad/resources/tinrs.png"), false) ?;

    shell_env_cap(&[], &out_dir, &sdk_dir.join("android-13/aapt").to_str().unwrap(), &[
        "add",
        &dst_apk.to_str().unwrap(),
        "assets/makepad/makepad_widgets/resources/IBMPlexSans-Text.ttf",
        "assets/makepad/makepad_widgets/resources/IBMPlexSans-SemiBold.ttf",
        "assets/makepad/makepad_widgets/resources/LiberationMono-Regular.ttf",
        "assets/makepad/resources/tinrs.png",
    ]) ?;
        
    let java_home = sdk_dir.join("openjdk");
    shell_env_cap(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &sdk_dir.join("android-13/apksigner").to_str().unwrap(),
        &[
            "sign",
            "-v",
            "-ks",
            "tools/android/debug.keystore",
            "--ks-key-alias",
            "androiddebugkey",
            "--ks-pass",
            "pass:android",
            &dst_apk.to_str().unwrap() 
        ]
    ) ?;
    
    /*shell_env(&[], &cwd, &sdk_dir.join("android-13/aapt").to_str().unwrap(), &[
        "list",
        &dst_apk.to_str().unwrap(),
    ]) ?;*/
    Ok(dst_apk)
}

pub fn run(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<(), String> {
    let dst_apk = build(sdk_dir, host_os, args)?;
    let cwd = std::env::current_dir().unwrap();
    println!("Installing android application");
    shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "install",
        "-r",
        &dst_apk.to_str().unwrap(),
    ]) ?;
    println!("Starting android application");
    shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "shell",
        "am",
        "start",
        "-n",
        "nl.makepad.android/nl.makepad.android.MakepadActivity"
    ]) ?;
    #[allow(unused_assignments)]
    let mut pid = None;
    loop{
        if let Ok(thing) = shell_env_cap(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
            "shell",
            "pidof",
            "nl.makepad.android",
        ]){
            pid = Some(thing.trim().to_string());
            break;
        }
    }
    shell_env(&[], &cwd, &sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
        "logcat",
        "--pid",
        &pid.unwrap(),
        "*:S Makepad:D"
    ]) ?;
    Ok(())
}


pub fn base_apk(sdk_dir: &Path, _host_os: HostOs, _args: &[String]) -> Result<(), String> {
    // lets compile the makepad base apk and write it to the sdk dir
    let cwd = std::env::current_dir().unwrap();
    let out_dir = cwd.join("target/aarch64-linux-android-apk/makepad_base_apk");
    mkdir(&out_dir) ?;
    let java_home = sdk_dir.join("openjdk");
    println!("Compiling makepad java");
    shell_env(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &java_home.join("bin/javac").to_str().unwrap(),
        &[
            "-classpath",
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "-Xlint:deprecation",
            "-d",
            &out_dir.to_str().unwrap(),
            &cwd.join("platform/src/os/linux/android/java/nl/makepad/android/Makepad.java").to_str().unwrap(),
            &cwd.join("platform/src/os/linux/android/java/nl/makepad/android/MakepadActivity.java").to_str().unwrap(),
            &cwd.join("platform/src/os/linux/android/java/nl/makepad/android/MakepadSurfaceView.java").to_str().unwrap()
        ]
    ) ?;
    println!("Building dex file");
    shell_env(
        &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &sdk_dir.join("android-13/d8").to_str().unwrap(),
        &[
            "--classpath",
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "--output",
            &out_dir.to_str().unwrap(),
            &out_dir.join("nl/makepad/android/Makepad.class").to_str().unwrap(),
            &out_dir.join("nl/makepad/android/MakepadActivity.class").to_str().unwrap(),
            &out_dir.join("nl/makepad/android/MakepadSurfaceView.class").to_str().unwrap(),
            &out_dir.join("nl/makepad/android/Makepad$Callback.class").to_str().unwrap()
        ]
    ) ?;
    println!("Creating base apk file");
    shell_env(
        &[],
        &cwd,
        &sdk_dir.join("android-13/aapt").to_str().unwrap(),
        &[
            "package",
            "-f",
            "-F",
            &out_dir.join("makepad_base_apk.apk").to_str().unwrap(),
            "-I",
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "-M",
            &cwd.join("platform/src/os/linux/android/xml/AndroidManifest.xml").to_str().unwrap(),
            &out_dir.to_str().unwrap(),
        ]
    ) ?;
    /*println!("Adding classes.dex to base apk file");
    shell_env( 
        &[],
        &out_dir, 
        &sdk_dir.join("android-13/aapt").to_str().unwrap(),
        &[
            "add",
            &out_dir.join("makepad_base_apk.apk").to_str().unwrap(),
            "classes.dex",
        ]
    )?;*/
    println!("Base apk file completed");
    Ok(())
}
