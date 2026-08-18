use std::env;
use std::fs;
use std::process::Command;

const IOS_DEPLOYMENT_TARGET_DEFAULT: &str = "26.0";

fn main() {
    println!("cargo:rerun-if-changed=swift/tts_bridge.swift");
    println!("cargo:rustc-check-cfg=cfg(no_apple_tts)");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let is_apple_host = env::var("HOST").unwrap_or_default().contains("apple");
    let is_apple_target = target_os == "macos" || target_os == "ios";

    // No Swift toolchain, or not an Apple target: the crate degrades to a no-op
    // rather than failing the build.
    if !(is_apple_host && is_apple_target) || !build_tts_bridge(&target_os) {
        println!("cargo:rustc-cfg=no_apple_tts");
    }
}

/// Compile `swift/tts_bridge.swift` into a static library and link it.
///
/// Unlike the speech (STT) bridge, nothing here is `async`, so Swift Concurrency
/// is never linked and the `@rpath/libswift_Concurrency.dylib` install-name
/// workaround that `makepad-voice` needs does not apply.
fn build_tts_bridge(target_os: &str) -> bool {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let swift_src = format!("{manifest_dir}/swift/tts_bridge.swift");
    let module_cache = format!("{out_dir}/swift_module_cache");
    let _ = fs::create_dir_all(&module_cache);

    let mut args = vec![
        "-emit-library".to_string(),
        "-static".to_string(),
        "-parse-as-library".to_string(),
        "-module-name".to_string(),
        "tts_bridge".to_string(),
        "-module-cache-path".to_string(),
        module_cache,
        "-O".to_string(),
    ];
    if target_os == "ios" {
        if let Some((target, sdk)) = ios_target_and_sdk() {
            args.push("-target".to_string());
            args.push(target);
            args.push("-sdk".to_string());
            args.push(sdk);
        }
    }
    args.push("-o".to_string());
    args.push(format!("{out_dir}/libtts_bridge.a"));
    args.push(swift_src);

    match Command::new("swiftc").args(&args).status() {
        Ok(status) if status.success() => {}
        Ok(_) => {
            println!("cargo:warning=swiftc failed for tts bridge; speech is disabled");
            return false;
        }
        Err(err) => {
            println!("cargo:warning=swiftc unavailable ({err}); speech is disabled");
            return false;
        }
    }

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=tts_bridge");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");

    // Let the linker resolve the Swift runtime symbols the bridge pulls in.
    if let Ok(output) = Command::new("swiftc").args(["-print-target-info"]).output() {
        if output.status.success() {
            let info = String::from_utf8_lossy(&output.stdout);
            for line in info.lines() {
                let path = line.trim().trim_matches('"').trim_end_matches(',');
                if path.starts_with('/') && path.contains("lib/swift") {
                    println!("cargo:rustc-link-search=native={path}");
                }
            }
        }
    }

    true
}

fn ios_target_and_sdk() -> Option<(String, String)> {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").ok()?;
    let abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    let is_simulator = abi == "sim" || arch == "x86_64";
    let swift_arch = match arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    let deployment_key = if is_simulator {
        "IPHONESIMULATOR_DEPLOYMENT_TARGET"
    } else {
        "IPHONEOS_DEPLOYMENT_TARGET"
    };
    let deployment =
        env::var(deployment_key).unwrap_or_else(|_| IOS_DEPLOYMENT_TARGET_DEFAULT.to_string());
    let swift_target = if is_simulator {
        format!("{swift_arch}-apple-ios{deployment}-simulator")
    } else {
        format!("{swift_arch}-apple-ios{deployment}")
    };
    let sdk_name = if is_simulator {
        "iphonesimulator"
    } else {
        "iphoneos"
    };
    let sdk_path = Command::new("xcrun")
        .args(["--sdk", sdk_name, "--show-sdk-path"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())?;
    Some((swift_target, sdk_path))
}
