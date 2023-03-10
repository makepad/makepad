use std::{
    path::{Path, PathBuf},
};
use crate::android::HostOs;
use crate::android::shell::*;

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

pub struct BuildResult{
    dst_apk: PathBuf,
    java_url: String,
}

pub fn build(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<BuildResult, String> {
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
    
    // main names used in the process
    let underscore_target = target.replace("-", "_");
    let java_url = format!("dev.{underscore_target}.app");
    let app_label = format!("{underscore_target}");

    // alright lets do the rust stuff.
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
    
    let tmp_dir = cwd.join(format!("target/aarch64-linux-android-apk/{underscore_target}/tmp"));
    let out_dir = cwd.join(format!("target/aarch64-linux-android-apk/{underscore_target}/apk"));
    
    // lets remove tmp and out dir
    let _ = rmdir(&tmp_dir);
    let _ = rmdir(&out_dir);
    mkdir(&tmp_dir) ?; 
    mkdir(&out_dir) ?;
    // alright lets go and generate the root java file
    // and the manifest xml
    // then build the dst_apk
    // we'll leave the classname as is
    let manifest_xml = manifest_xml(&app_label,"MakepadApp",&java_url);
    let manifest_file = tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;
    
    let main_java = main_java(&java_url);
    let java_path = java_url.replace(".","/");
    let java_file = tmp_dir.join(&java_path).join("MakepadApp.java");
    let java_class = out_dir.join(&java_path).join("MakepadApp.class");
    write_text(&java_file, &main_java)?;
    
    // lets build the APK
    let java_home = sdk_dir.join("openjdk");
    println!("Compiling APK file");
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
            &cwd.join("platform/src/os/linux/android/java/dev/makepad/android/Makepad.java").to_str().unwrap(),
            &cwd.join("platform/src/os/linux/android/java/dev/makepad/android/MakepadActivity.java").to_str().unwrap(),
            &cwd.join("platform/src/os/linux/android/java/dev/makepad/android/MakepadSurfaceView.java").to_str().unwrap(),
            &java_file.to_str().unwrap()
        ]
    ) ?;

    //println!("Building dex file");
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
            &out_dir.join("dev/makepad/android/Makepad.class").to_str().unwrap(),
            &out_dir.join("dev/makepad/android/MakepadActivity.class").to_str().unwrap(),
            &out_dir.join("dev/makepad/android/MakepadSurfaceView.class").to_str().unwrap(),
            &out_dir.join("dev/makepad/android/Makepad$Callback.class").to_str().unwrap(),
            &java_class.to_str().unwrap(),
        ]
    ) ?;

    let dst_apk = out_dir.join(format!("{underscore_target}.apk"));
    
    //println!("Creating base apk file");
    let _ = rm(&dst_apk); 
    shell_env(
         &[("JAVA_HOME", &java_home.to_str().unwrap())],
        &cwd,
        &sdk_dir.join("android-13/aapt").to_str().unwrap(),
        &[
            "package",
            "-f",
            "-F",
            &dst_apk.to_str().unwrap(),
            "-I",
            &sdk_dir.join("android-33-ext4/android.jar").to_str().unwrap(),
            "-M",
            &manifest_file.to_str().unwrap(),
            &out_dir.to_str().unwrap(),
        ]
    ) ?;
    
    //println!("Adding library");
     
    mkdir(&out_dir.join("lib/arm64-v8a")) ?;
    
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
        &java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            &sdk_dir.join("android-13/lib/apksigner.jar").to_str().unwrap(),
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
    println!("Compile APK completed");
    Ok(BuildResult{
        dst_apk,
        java_url,
    })
}

pub fn run(sdk_dir: &Path, host_os: HostOs, args: &[String]) -> Result<(), String> {
    let result = build(sdk_dir, host_os, args)?;
    
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
