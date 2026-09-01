//! Builds the Apple bridge (`swift/stt_bridge.swift` + `swift/tts_bridge.swift`)
//! into one static library on Apple hosts targeting macOS/iOS. Without a Swift
//! toolchain, or off Apple, the crate compiles without the `apple_speech` cfg
//! and both engines report themselves unavailable.

use std::env;
use std::fs;
use std::process::Command;

/// SpeechAnalyzer (the STT half of the bridge) is iOS 26+.
const IOS_DEPLOYMENT_TARGET_DEFAULT: &str = "26.0";

fn main() {
    println!("cargo:rustc-check-cfg=cfg(apple_speech)");
    println!("cargo:rerun-if-changed=swift/stt_bridge.swift");
    println!("cargo:rerun-if-changed=swift/tts_bridge.swift");
    println!("cargo:rerun-if-env-changed=MAKEPAD_SYSTEM_SPEECH_NO_APPLE_BRIDGE");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    println!("cargo:rerun-if-env-changed=IPHONESIMULATOR_DEPLOYMENT_TARGET");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let is_apple_host = env::var("HOST").unwrap_or_default().contains("apple-darwin");
    let is_apple_target = target_os == "macos" || target_os == "ios";
    let disabled = env::var_os("MAKEPAD_SYSTEM_SPEECH_NO_APPLE_BRIDGE").is_some();

    if is_apple_host && is_apple_target && !disabled && build_apple_bridge(&target_os) {
        println!("cargo:rustc-cfg=apple_speech");
    } else if is_apple_target && !disabled {
        println!("cargo:warning=makepad-system-speech: Apple bridge not built; system STT/TTS unavailable");
    }
}

fn build_apple_bridge(target_os: &str) -> bool {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let module_cache = format!("{out_dir}/swift_module_cache");
    let _ = fs::create_dir_all(&module_cache);

    let mut args = vec![
        "-emit-library".to_string(),
        "-static".to_string(),
        "-parse-as-library".to_string(),
        "-module-name".to_string(),
        "makepad_system_speech".to_string(),
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
    args.push(format!("{out_dir}/libmakepad_system_speech.a"));
    args.push(format!("{manifest_dir}/swift/stt_bridge.swift"));
    args.push(format!("{manifest_dir}/swift/tts_bridge.swift"));

    match Command::new("swiftc").args(&args).status() {
        Ok(status) if status.success() => {}
        Ok(_) => {
            println!("cargo:warning=swiftc failed for the makepad-system-speech Apple bridge");
            return false;
        }
        Err(err) => {
            println!("cargo:warning=swiftc unavailable ({err}) for the makepad-system-speech Apple bridge");
            return false;
        }
    }

    // Must come BEFORE any other link-search line so the patched .tbd files win.
    if target_os == "macos" {
        fix_swift_rpath_tbds(&out_dir);
    }

    println!("cargo:rustc-link-search=native={out_dir}");
    println!("cargo:rustc-link-lib=static=makepad_system_speech");

    // The Swift objects were built for this deployment target; the Rust link
    // step must agree or the async runtime's symbols go missing.
    if target_os == "ios" {
        let deployment = ios_deployment_target();
        println!("cargo:rustc-link-arg=-miphoneos-version-min={deployment}");
    }

    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");

    // Swift runtime search paths so the linker resolves the bridge's symbols.
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

fn ios_is_simulator() -> bool {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let abi = env::var("CARGO_CFG_TARGET_ABI").unwrap_or_default();
    abi == "sim" || arch == "x86_64"
}

fn ios_deployment_target() -> String {
    let key = if ios_is_simulator() {
        "IPHONESIMULATOR_DEPLOYMENT_TARGET"
    } else {
        "IPHONEOS_DEPLOYMENT_TARGET"
    };
    env::var(key).unwrap_or_else(|_| IOS_DEPLOYMENT_TARGET_DEFAULT.to_string())
}

fn ios_target_and_sdk() -> Option<(String, String)> {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").ok()?;
    let swift_arch = match arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return None,
    };
    let deployment = ios_deployment_target();
    let (swift_target, sdk_name) = if ios_is_simulator() {
        (format!("{swift_arch}-apple-ios{deployment}-simulator"), "iphonesimulator")
    } else {
        (format!("{swift_arch}-apple-ios{deployment}"), "iphoneos")
    };
    let sdk_path = Command::new("xcrun")
        .args(["--sdk", sdk_name, "--show-sdk-path"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())?;
    Some((swift_target, sdk_path))
}

/// The macOS SDK's Swift runtime `.tbd` files carry `$ld$previous` entries that
/// make the linker record `@rpath/libswift_Concurrency.dylib` as the install
/// name whenever MACOSX_DEPLOYMENT_TARGET < 15 — and Rust defaults to 11.0. The
/// binary then dies at launch with "Library not loaded: @rpath/...". Patched
/// copies without those entries, searched first, make the linker use the
/// absolute `/usr/lib/swift/...` names instead.
fn fix_swift_rpath_tbds(out_dir: &str) {
    let Some(sdk_path) = Command::new("xcrun")
        .args(["--show-sdk-path"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    else {
        return;
    };
    let override_dir = format!("{out_dir}/swift_tbd_override");
    if fs::create_dir_all(&override_dir).is_err() {
        return;
    }
    let swift_tbd_dir = format!("{sdk_path}/usr/lib/swift");
    for name in [
        "libswift_Concurrency.tbd",
        "libswiftCore.tbd",
        "libswiftFoundation.tbd",
        "libswift_StringProcessing.tbd",
        "libswift_RegexParser.tbd",
    ] {
        let Ok(content) = fs::read_to_string(format!("{swift_tbd_dir}/{name}")) else {
            continue;
        };
        if !content.contains("$ld$previous$@rpath/") {
            continue;
        }
        let _ = fs::write(format!("{override_dir}/{name}"), strip_ld_previous_rpath(&content));
    }
    println!("cargo:rustc-link-search=native={override_dir}");
}

fn strip_ld_previous_rpath(content: &str) -> String {
    let mut result = content.to_string();
    while let Some(start) = result.find("'$ld$previous$@rpath/") {
        let Some(end_quote_offset) = result[start + 1..].find('\'') else {
            break;
        };
        let end = start + 1 + end_quote_offset + 1;
        let rest = &result[end..];
        let trimmed = rest.trim_start_matches(|c: char| matches!(c, ',' | ' ' | '\n' | '\r'));
        let skip = rest.len() - trimmed.len();
        result = format!("{}{}", &result[..start], &result[end + skip..]);
    }
    result
}
