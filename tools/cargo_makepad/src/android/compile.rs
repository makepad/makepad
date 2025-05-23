use std::path::{Path, PathBuf};
use crate::android::{HostOs, AndroidTarget, AndroidVariant};
use crate::utils::*;
use crate::makepad_shell::*;
use super::sdk::{AndroidSDKUrls,  BUILD_TOOLS_DIR, PLATFORMS_DIR};

fn aapt_path(sdk_dir: &Path, urls:&AndroidSDKUrls) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(urls.build_tools_version).join("aapt")
}

fn d8_jar_path(sdk_dir: &Path, urls:&AndroidSDKUrls) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(urls.build_tools_version).join("lib/d8.jar")
}

fn apksigner_jar_path(sdk_dir: &Path, urls:&AndroidSDKUrls) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(urls.build_tools_version).join("lib/apksigner.jar")
}

fn zipalign_path(sdk_dir: &Path, urls:&AndroidSDKUrls) -> PathBuf {
    sdk_dir.join(BUILD_TOOLS_DIR).join(urls.build_tools_version).join("zipalign")
}

fn android_jar_path(sdk_dir: &Path, urls:&AndroidSDKUrls) -> PathBuf {
    sdk_dir.join(PLATFORMS_DIR).join(urls.platform).join("android.jar")
}

#[derive(Debug)]
struct BuildPaths {
    tmp_dir: PathBuf,
    out_dir: PathBuf,
    manifest_file: PathBuf,
    java_file: PathBuf,
    java_class: PathBuf,
    xr_class: PathBuf,
    xr_file: PathBuf,
    dst_unaligned_apk: PathBuf,
    dst_apk: PathBuf,
}

pub struct BuildResult {
    dst_apk: PathBuf,
    java_url: String,
}



fn main_java(url:&str)->String{
    format!(r#"
        package {url};
        import dev.makepad.android.MakepadActivity;
        public class MakepadApp extends MakepadActivity{{
            public void switchActivity(){{
                switchActivityClass(MakepadAppXr.class);
            }}
        }}
    "#)
}

fn xr_java(url:&str)->String{
    format!(r#"
        package {url};
        import dev.makepad.android.MakepadActivity;
        public class MakepadAppXr extends MakepadActivity{{
            public void switchActivity(){{
                switchActivityClass(MakepadApp.class);
            }}
        }}
    "#)
}


fn rust_build(sdk_dir: &Path, host_os: HostOs, args: &[String], android_targets:&[AndroidTarget], variant:&AndroidVariant, urls:&AndroidSDKUrls) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    #[allow(non_snake_case)]
    let NDK_VERSION_FULL = urls.ndk_version_full;
    for android_target in android_targets {
        let clang_filename = format!("{}33-clang", android_target.clang());
        
        let bin_path = |bin_filename: &str, windows_extension: &str| match host_os {
            HostOs::MacosX64     => format!("ndk/{NDK_VERSION_FULL}/toolchains/llvm/prebuilt/darwin-x86_64/bin/{bin_filename}"),
            // NOTE! The NDK version we install (33) does not have darwin-aarch64 binaries (host) so we have to use darwin-x86
            // This automatically runs via rosetta on M1 and newer targets.
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
         
        let makepad_env = std::env::var("MAKEPAD").unwrap_or("lines".to_string());
        let makepad_env = if let AndroidVariant::Quest = variant{
            format!("{}+quest", makepad_env)
        }
        else{
            makepad_env
        };
        
        shell_env(
            &[
                // Set the linker env var to the path of the target-specific `clang` binary.
                (&android_target.linker_env_var(), full_clang_path.to_str().unwrap()),

                // We set standard Android-related env vars to allow other tools to know
                // which version of Java, Android build tools, and Android SDK we're targeting.
                // These environment variables are either standard in the Android ecosystem 
                // or are defined by the `android-build` crate: <https://crates.io/crates/android-build>.
                ("ANDROID_HOME",                 sdk_dir.to_str().unwrap()),
                ("ANDROID_SDK_ROOT",             sdk_dir.to_str().unwrap()),
                ("ANDROID_BUILD_TOOLS_VERSION",  urls.build_tools_version),
                ("ANDROID_PLATFORM",             urls.platform),
                ("ANDROID_SDK_VERSION",          urls.sdk_version.to_string().as_str()),
                ("ANDROID_API_LEVEL",            urls.sdk_version.to_string().as_str()),  // for legacy/clarity purposes
                ("ANDROID_SDK_EXTENSION",        urls.sdk_extension),
                ("JAVA_HOME",                    sdk_dir.join("openjdk").to_str().unwrap()),

                // We set these three env vars to allow native library C/C++ builds to succeed with no additional app-side config.
                // The naming conventions of these env variable keys are established by the `cc` Rust crate.
                (&format!("CC_{toolchain}"),     full_clang_path.to_str().unwrap()),
                (&format!("AR_{toolchain}"),     full_llvm_ar_path.to_str().unwrap()),
                (&format!("RANLIB_{toolchain}"), full_llvm_ranlib_path.to_str().unwrap()),

                ("RUSTFLAGS", &cfg_flag),
                ("MAKEPAD", &makepad_env),
            ],
            &cwd,
            "rustup",
            &args_out
        ) ?;
    }

    Ok(())
}

fn prepare_build(underscore_build_crate: &str, java_url: &str, app_label: &str, variant:&AndroidVariant, urls:&AndroidSDKUrls) -> Result<BuildPaths, String> {
    let cwd = std::env::current_dir().unwrap();

    let tmp_dir = cwd.join("target").join("makepad-android-apk").join(&underscore_build_crate).join("tmp");
    let out_dir = cwd.join("target").join("makepad-android-apk").join(&underscore_build_crate).join("apk");
    
    // lets remove tmp and out dir
    let _ = rmdir(&tmp_dir);
    let _ = rmdir(&out_dir);
    mkdir(&tmp_dir) ?; 
    mkdir(&out_dir) ?;

    let manifest_xml = variant.manifest_xml(app_label, "MakepadApp", java_url, urls.sdk_version);
    let manifest_file = tmp_dir.join("AndroidManifest.xml");
    write_text(&manifest_file, &manifest_xml)?;
    
    let main_java = main_java(java_url);
    let java_path = java_url.replace('.',"/");
    let java_file = tmp_dir.join(&java_path).join("MakepadApp.java");
    let java_class = out_dir.join(&java_path).join("MakepadApp.class");
    write_text(&java_file, &main_java)?;
        
    let xr_java = xr_java(java_url);
    let xr_file = tmp_dir.join(&java_path).join("MakepadAppXr.java");
    let xr_class = out_dir.join(&java_path).join("MakepadAppXr.class");
    write_text(&xr_file, &xr_java)?;
    
        
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
        xr_class,
        xr_file,
        dst_unaligned_apk,
        dst_apk,
    })
}

fn build_r_class(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
    let java_home = sdk_dir.join("openjdk");
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
       &cwd,
       &aapt_path(sdk_dir, urls).to_str().unwrap(),
       &[
           "package",
           "-f",
           "-m",
           "-I",
           (android_jar_path(sdk_dir, urls).to_str().unwrap()),
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

fn compile_java(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
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
            (android_jar_path(sdk_dir, urls).to_str().unwrap()),
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
            (build_paths.java_file.to_str().unwrap()),
            (build_paths.xr_file.to_str().unwrap()),
        ]   
    ) ?; 

    Ok(())
}

fn build_dex(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
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
            (d8_jar_path(sdk_dir, urls).to_str().unwrap()),
            "com.android.tools.r8.D8",
            "--classpath",
            (android_jar_path(sdk_dir, urls).to_str().unwrap()),
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
            (build_paths.xr_class.to_str().unwrap()),
        ]
    ) ?;

    Ok(())
} 

fn build_unaligned_apk(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let java_home = sdk_dir.join("openjdk");
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    shell_env(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
       &cwd,
       aapt_path(sdk_dir, urls).to_str().unwrap(),
       &[ 
           "package",
           "-f",
           "-F",
           (build_paths.dst_unaligned_apk.to_str().unwrap()),
           "-I",
           (android_jar_path(sdk_dir, urls).to_str().unwrap()),
           "-M",
           (build_paths.manifest_file.to_str().unwrap()),
           "-S",
           (cargo_manifest_dir.join("src/android/res").to_str().unwrap()),
           (build_paths.out_dir.to_str().unwrap()),
       ]
    ) ?;

    Ok(())
}

fn add_rust_library(sdk_dir: &Path, underscore_target: &str, build_paths: &BuildPaths, android_targets: &[AndroidTarget], args: &[String], variant:&AndroidVariant, urls:&AndroidSDKUrls) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().unwrap();
    let profile = get_profile_from_args(args);
    let mut build_dir = None;
    for android_target in android_targets {
        let abi = android_target.abi_identifier();
        mkdir(&build_paths.out_dir.join(format!("lib/{abi}"))) ?;

        let android_target_dir = android_target.toolchain();
        let binary_path = format!("lib/{abi}/libmakepad.so");
        if profile == "debug"{
            println!("WARNING - compiling a DEBUG build of the application, this creates a very slow and big app. Try adding --release for a fast, or --profile=small for a small build.");
        }
        let src_lib = cwd.join(format!("target/{android_target_dir}/{profile}/lib{underscore_target}.so"));
        build_dir = Some(cwd.join(format!("target/{android_target_dir}/{profile}")));
        let dst_lib = build_paths.out_dir.join(binary_path.clone());
        cp(&src_lib, &dst_lib, false) ?;

        shell_env_cap(&[], &build_paths.out_dir, aapt_path(sdk_dir, urls).to_str().unwrap(), &[
            "add",
            (build_paths.dst_unaligned_apk.to_str().unwrap()),
            &binary_path,
        ]) ?;
    }
    // for the quest variant add the precompiled openXR loader
    if let AndroidVariant::Quest = variant{
        
        let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        
        for (binary_path, src_lib) in [
            ("lib/arm64-v8a/libopenxr_loader.so", "quest/libopenxr_loader.so"),
            //("lib/arm64-v8a/libktx.so", "tools/cargo_makepad/quest/libktx.so"),
            //("lib/arm64-v8a/libktx_read.so", "tools/cargo_makepad/quest/libktx_read.so"),
            //("lib/arm64-v8a/libobjUtil.a", "tools/cargo_makepad/quest/libobjUtil.a"),
        ]{
            //let binary_path = format!("lib/arm64-v8a/libopenxr_loader.so");
            let src_lib = cargo_manifest_dir.join(src_lib);
            let dst_lib = build_paths.out_dir.join(binary_path);
            cp(&src_lib, &dst_lib, false) ?;
            shell_env_cap(&[], &build_paths.out_dir, aapt_path(sdk_dir, urls).to_str().unwrap(), &[
                "add",
                (build_paths.dst_unaligned_apk.to_str().unwrap()),
                &binary_path,
            ]) ?;
                        
        }
    }

    Ok(build_dir.unwrap())
}

fn add_resources(sdk_dir: &Path, build_crate: &str, build_paths: &BuildPaths, build_dir:&Path, android_targets:&[AndroidTarget], variant:&AndroidVariant, urls:&AndroidSDKUrls) -> Result<(), String> {
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

    let deps = get_crate_dep_dirs(build_crate, &build_dir, &android_targets[0].toolchain());
    for (name, dep_dir) in deps.iter() {
        let resources_path = dep_dir.join("resources");
        if resources_path.is_dir(){
            let name = name.replace('-', "_");
            
            let dst_dir = build_paths.out_dir.join(format!("assets/makepad/{name}/resources"));
            mkdir(&dst_dir) ?;
            cp_all(&resources_path, &dst_dir, false) ?;
    
            let assets = ls(&dst_dir) ?;
            for path in &assets {
                let path = path.display().to_string();
                assets_to_add.push(format!("assets/makepad/{name}/resources/{path}"));
            }
        }
    }
    // FIX THIS PROPER
    // On quest remove most of the widget resourcse
    if let AndroidVariant::Quest = variant{
        let dst_dir = build_paths.out_dir.join(format!("assets/makepad/makepad_widgets/resources"));
        let remove = [
            "fa-solid-900.ttf",
            //"LXGWWenKaiBold.ttf",
            "LiberationMono-Regular.ttf",
            "GoNotoKurrent-Bold.ttf",
           // "NotoColorEmoji.ttf",
            //"IBMPlexSans-SemiBold.ttf",
            "NotoSans-Regular.ttf",
        ];
        for remove in remove{
            assets_to_add.retain(|v| !v.contains(remove));
            rm(&dst_dir.join(remove))?;
        }
    }

    let mut aapt_args = vec![
        "add",
        build_paths.dst_unaligned_apk.to_str().unwrap(),
    ];
    for asset in &assets_to_add {  
        aapt_args.push(asset);
    }

    shell_env_cap(&[], &build_paths.out_dir, aapt_path(sdk_dir, urls).to_str().unwrap(), &aapt_args) ?;

    Ok(())
}

fn build_zipaligned_apk(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
    shell_env_cap(&[], &build_paths.out_dir, zipalign_path(sdk_dir, urls).to_str().unwrap(), &[
       "-v",
       "-f",
       "4",
       (build_paths.dst_unaligned_apk.to_str().unwrap()),
       (build_paths.dst_apk.to_str().unwrap()),
    ]) ?;

    Ok(())
}

fn sign_apk(sdk_dir: &Path, build_paths: &BuildPaths, urls:&AndroidSDKUrls) -> Result<(), String> {
    let cwd = std::env::current_dir().unwrap();
    let cargo_manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let java_home = sdk_dir.join("openjdk");

    shell_env_cap(
        &[("JAVA_HOME", (java_home.to_str().unwrap()))],
        &cwd,
        java_home.join("bin/java").to_str().unwrap(),
        &[
            "-jar",
            (apksigner_jar_path(sdk_dir, urls).to_str().unwrap()),
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

pub fn build(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String], android_targets:&[AndroidTarget], variant:&AndroidVariant, urls: &AndroidSDKUrls) -> Result<BuildResult, String> {
    let build_crate = get_build_crate_from_args(args)?;
    let underscore_build_crate = build_crate.replace('-', "_");

    let java_url = package_name.unwrap_or_else(|| format!("dev.makepad.{underscore_build_crate}"));
    let app_label = app_label.unwrap_or_else(|| format!("{underscore_build_crate}"));

    rust_build(sdk_dir, host_os, args, android_targets, variant, urls)?;
    let build_paths = prepare_build(build_crate, &java_url, &app_label, variant, urls)?;

    println!("Compiling APK & R.java files");
    build_r_class(sdk_dir, &build_paths, urls)?;
    compile_java(sdk_dir, &build_paths, urls)?;

    println!("Building APK");
    build_dex(sdk_dir, &build_paths, urls)?;
    build_unaligned_apk(sdk_dir, &build_paths, urls)?;
    let build_dir = add_rust_library(sdk_dir, &underscore_build_crate, &build_paths, android_targets, args, variant, urls)?;
    add_resources(sdk_dir, build_crate, &build_paths, &build_dir, android_targets, variant, urls)?;
    build_zipaligned_apk(sdk_dir, &build_paths, urls)?;
    sign_apk(sdk_dir, &build_paths, urls)?;

    println!("Compile APK completed");
    Ok(BuildResult{
        dst_apk: build_paths.dst_apk,
        java_url,
    })
}

pub fn run(sdk_dir: &Path, host_os: HostOs, package_name: Option<String>, app_label: Option<String>, args: &[String], targets:&[AndroidTarget], android_variant:&AndroidVariant, urls:&AndroidSDKUrls, devices:Vec<String>) -> Result<(), String> {
    let result = build(sdk_dir, host_os, package_name, app_label, args, targets, android_variant, urls)?;
    
    let cwd = std::env::current_dir().unwrap();
    // alright so how will we do multiple targets eh
    
    if devices.len() == 0{
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
    }
    else{
        let mut children = Vec::new();
        for device in &devices{
            //println!("Installing android application");
            children.push(shell_child_create(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
                "-s",
                &device,
                "install",
                "-r",
                (result.dst_apk.to_str().unwrap()),
            ]) ?);
        }
        for child in children{
            shell_child_wait(child)?;
        }
        let mut children = Vec::new();
        for device in &devices{
            //println!("Installing android application");
            children.push(shell_child_create(&[], &cwd, sdk_dir.join("platform-tools/adb").to_str().unwrap(), &[
                "-s",
                &device,
                "shell",
                "am",
                "start",
                "-n",
                &format!("{0}/{0}.MakepadApp", result.java_url)
            ]) ?);
        }
        for child in children{
            shell_child_wait(child)?;
        }
    }
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
