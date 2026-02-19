use std::env;
use std::fs;
use std::process::Command;

const IOS_DEPLOYMENT_TARGET_DEFAULT: &str = "26.0";

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let force_whisper = env::var("MAKEPAD")
        .ok()
        .is_some_and(|configs| configs.split(['+', ',']).any(|config| config == "whisper"));

    println!("cargo:rustc-check-cfg=cfg(force_whisper)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    println!("cargo:rerun-if-env-changed=IPHONESIMULATOR_DEPLOYMENT_TARGET");
    if force_whisper {
        println!("cargo:rustc-cfg=force_whisper");
    }

    if force_whisper || !host.contains("apple-darwin") {
        return;
    }
    if target_os == "macos" || target_os == "ios" {
        build_speech_bridge(&target_os);
    }
}

fn build_speech_bridge(target_os: &str) {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let swift_src = format!("{}/swift/speech_bridge.swift", manifest_dir);
    let swift_module_cache = format!("{}/swift_module_cache", out_dir);

    println!("cargo:rerun-if-changed=swift/speech_bridge.swift");
    let _ = fs::create_dir_all(&swift_module_cache);

    // Compile Swift -> static library
    let output_lib = format!("{}/libspeech_bridge.a", out_dir);
    let mut swift_args = vec![
        "-emit-library".to_string(),
        "-static".to_string(),
        "-parse-as-library".to_string(),
        "-module-name".to_string(),
        "speech_bridge".to_string(),
        "-module-cache-path".to_string(),
        swift_module_cache.clone(),
    ];
    if target_os == "ios" {
        if let Some((swift_target, sdk_path)) = ios_swift_target_and_sdk() {
            swift_args.push("-target".to_string());
            swift_args.push(swift_target);
            swift_args.push("-sdk".to_string());
            swift_args.push(sdk_path);
        }
    }
    swift_args.push("-o".to_string());
    swift_args.push(output_lib.clone());
    swift_args.push(swift_src);

    let status = Command::new("swiftc")
        .args(swift_args)
        .status()
        .expect("failed to run swiftc — is Xcode command line tools installed?");

    if !status.success() {
        panic!("swiftc compilation failed");
    }

    // Fix the @rpath issue with libswift_Concurrency.dylib BEFORE emitting any
    // link-search paths, so our override directory appears first in -L order.
    //
    // Background: The macOS SDK's libswift_Concurrency.tbd contains $ld$previous
    // entries that tell the linker to record "@rpath/libswift_Concurrency.dylib"
    // as the install name when MACOSX_DEPLOYMENT_TARGET < 15.0. Rust defaults to
    // MACOSX_DEPLOYMENT_TARGET=11.0, which triggers this behavior. The resulting
    // binary then fails at runtime with "dyld: Library not loaded: @rpath/...".
    //
    // The fix: create modified copies of the .tbd files with $ld$previous entries
    // stripped, and add them to the linker search path before the SDK paths.
    // Since cargo:rustc-link-search propagates from library crates to dependent
    // binaries, the final binary's link step will find our clean .tbd first and
    // use the absolute install name "/usr/lib/swift/libswift_Concurrency.dylib".
    if target_os == "macos" {
        fix_swift_rpath_tbds(&out_dir);
    }

    // Link the static library
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=speech_bridge");

    // Link Apple frameworks
    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");

    // Add Swift runtime library search paths so the linker can resolve symbols.
    let target_info = Command::new("swiftc")
        .args(&["-print-target-info"])
        .output()
        .expect("failed to get swift target info");

    if target_info.status.success() {
        let info_str = String::from_utf8_lossy(&target_info.stdout);
        for line in info_str.lines() {
            let trimmed = line.trim().trim_matches('"').trim_end_matches(',');
            if trimmed.starts_with("/") && trimmed.contains("lib/swift") {
                println!("cargo:rustc-link-search=native={}", trimmed);
            }
        }
    }
}

fn ios_swift_target_and_sdk() -> Option<(String, String)> {
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
    let deployment = env::var(deployment_key)
        .unwrap_or_else(|_| IOS_DEPLOYMENT_TARGET_DEFAULT.to_string());
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
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    Some((swift_target, sdk_path))
}

/// Create modified copies of Swift runtime .tbd files without $ld$previous entries.
///
/// The $ld$previous entries in the SDK's .tbd files cause the linker to use
/// @rpath/ as the install name for deployment targets below certain thresholds.
/// By stripping these entries and placing our modified .tbd in a search directory
/// that comes before the SDK, the linker will use the actual absolute install
/// names (e.g. "/usr/lib/swift/libswift_Concurrency.dylib") regardless of the
/// deployment target.
#[cfg(target_os = "macos")]
fn fix_swift_rpath_tbds(out_dir: &str) {
    let sdk_path = Command::new("xcrun")
        .args(&["--show-sdk-path"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    let sdk_path = match sdk_path {
        Some(p) => p,
        None => return,
    };

    let override_dir = format!("{}/swift_tbd_override", out_dir);
    if fs::create_dir_all(&override_dir).is_err() {
        return;
    }

    // Only patch the specific Swift runtime .tbd files our static library depends on.
    let swift_tbd_dir = format!("{}/usr/lib/swift", sdk_path);
    let tbds_to_fix = [
        "libswift_Concurrency.tbd",
        "libswiftCore.tbd",
        "libswiftFoundation.tbd",
        "libswift_StringProcessing.tbd",
        "libswift_RegexParser.tbd",
    ];

    for name in &tbds_to_fix {
        let tbd_path = format!("{}/{}", swift_tbd_dir, name);
        let content = match fs::read_to_string(&tbd_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !content.contains("$ld$previous$@rpath/") {
            continue;
        }
        let fixed = strip_ld_previous_rpath(&content);
        let _ = fs::write(format!("{}/{}", override_dir, name), &fixed);
    }

    // Emit this search path BEFORE any other Swift library search paths.
    // cargo:rustc-link-search propagates from library crates to dependent binaries,
    // so the final binary's linker will find our modified .tbd files first.
    println!("cargo:rustc-link-search=native={}", override_dir);
}

/// Strip $ld$previous entries that reference @rpath/ from a .tbd file's content.
#[cfg(target_os = "macos")]
fn strip_ld_previous_rpath(content: &str) -> String {
    let mut result = content.to_string();

    // $ld$previous entries appear as quoted strings in YAML:
    // '$ld$previous$@rpath/libswift_Concurrency.dylib$$1$10.9$12.0$$'
    // They may be followed by comma + whitespace in a YAML sequence.
    while let Some(start) = result.find("'$ld$previous$@rpath/") {
        if let Some(end_quote_offset) = result[start + 1..].find('\'') {
            let end = start + 1 + end_quote_offset + 1; // past closing quote
            // Skip trailing comma and whitespace/newlines
            let rest = &result[end..];
            let trimmed = rest.trim_start_matches(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r');
            let skip = rest.len() - trimmed.len();
            result = format!("{}{}", &result[..start], &result[end + skip..]);
        } else {
            break;
        }
    }

    result
}
