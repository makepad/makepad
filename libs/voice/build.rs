use std::env;
use std::fs;
use std::process::Command;

const IOS_DEPLOYMENT_TARGET_DEFAULT: &str = "26.0";

fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let is_apple_host = host.contains("apple-darwin");
    let force_whisper = env::var("MAKEPAD")
        .ok()
        .is_some_and(|configs| configs.split(['+', ',']).any(|config| config == "whisper"));

    println!("cargo:rustc-check-cfg=cfg(force_whisper)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=MAKEPAD_VOICE_METAL_PRECOMPILE");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    println!("cargo:rerun-if-env-changed=IPHONESIMULATOR_DEPLOYMENT_TARGET");
    println!("cargo:rerun-if-env-changed=MAKEPAD_NO_VOICE");

    if force_whisper {
        println!("cargo:rustc-cfg=force_whisper");
    }

    // Always create the metallib env var (even if empty)
    let out_dir = env::var("OUT_DIR").unwrap();
    let metallib_path = format!("{}/ggml-default.metallib", out_dir);
    let _ = fs::create_dir_all(&out_dir);
    let _ = fs::write(&metallib_path, []); // Create empty file
    println!("cargo:rustc-env=MAKEPAD_VOICE_GGML_METALLIB={}", metallib_path);

    // Check if we should skip the full build
    let skip_voice = env::var("MAKEPAD_NO_VOICE").is_ok()
        || (target_os == "macos" && !is_macos_26_or_later());

    if skip_voice && (target_os == "macos" || target_os == "ios") {
        println!("cargo:warning=makepad-voice: skipping speech bridge (requires macOS 26+)");
        build_stub_speech_bridge(&out_dir);
        return;
    }

    if target_os == "macos" {
        build_whisper_metallib();
    }

    if force_whisper || !is_apple_host {
        return;
    }
    if target_os == "macos" || target_os == "ios" {
        build_speech_bridge(&target_os);
    }
}

fn build_stub_speech_bridge(out_dir: &str) {
    // Create a C stub file matching the extern declarations in apple_speech.rs:
    // fn apple_speech_transcribe(samples: *const f32, sample_count: i64, lang: *const c_char, out_count: *mut i32, out_segments: *mut *mut c_void) -> i32;
    // fn apple_speech_free_segments(ptr: *mut c_void, count: i32);
    // fn apple_speech_ensure_model(lang: *const c_char) -> i32;

    let stub_c = format!("{}/speech_bridge_stub.c", out_dir);
    let stub_code = r#"
#include <stdint.h>
#include <stddef.h>

int32_t apple_speech_transcribe(
    const float* samples,
    int64_t sample_count,
    const char* lang,
    int32_t* out_count,
    void** out_segments
) {
    if (out_count) *out_count = 0;
    if (out_segments) *out_segments = NULL;
    return -1; // Return error code - not supported
}

void apple_speech_free_segments(void* ptr, int32_t count) {
    // No-op stub
    (void)ptr;
    (void)count;
}

int32_t apple_speech_ensure_model(const char* lang) {
    (void)lang;
    return -1; // Return error code - not supported
}
"#;

    fs::write(&stub_c, stub_code).expect("Failed to write stub C file");

    // Compile the stub
    let stub_obj = format!("{}/speech_bridge_stub.o", out_dir);
    let status = Command::new("cc")
        .args(["-c", "-O2", &stub_c, "-o", &stub_obj])
        .status()
        .expect("Failed to compile stub");

    if !status.success() {
        panic!("Failed to compile speech bridge stub");
    }

    // Create static library
    let stub_lib = format!("{}/libspeech_bridge.a", out_dir);
    let status = Command::new("ar")
        .args(["rcs", &stub_lib, &stub_obj])
        .status()
        .expect("Failed to create stub library");

    if !status.success() {
        panic!("Failed to create speech bridge stub library");
    }

    // Link the stub library
    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=speech_bridge");
}

fn is_macos_26_or_later() -> bool {
    let output = Command::new("sw_vers")
        .args(["-productVersion"])
        .output()
        .ok();

    if let Some(output) = output {
        if output.status.success() {
            let version = String::from_utf8_lossy(&output.stdout);
            let version = version.trim();
            if let Some(major) = version.split('.').next() {
                if let Ok(major_num) = major.parse::<u32>() {
                    return major_num >= 26;
                }
            }
        }
    }
    false
}

fn build_whisper_metallib() {
    let precompile_enabled = env::var("MAKEPAD_VOICE_METAL_PRECOMPILE")
        .ok()
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no" || v == "off")
        })
        .unwrap_or(true);

    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    let ggml_src_dir = format!("{}/../../local/whisper.cpp/ggml/src", manifest_dir);
    let ggml_metal_dir = format!(
        "{}/../../local/whisper.cpp/ggml/src/ggml-metal",
        manifest_dir
    );

    let metal_src = format!("{}/ggml-metal.metal", ggml_metal_dir);
    let common_h = format!("{}/ggml-common.h", ggml_src_dir);
    let impl_h = format!("{}/ggml-metal-impl.h", ggml_metal_dir);

    println!("cargo:rerun-if-changed={}", metal_src);
    println!("cargo:rerun-if-changed={}", common_h);
    println!("cargo:rerun-if-changed={}", impl_h);

    let _ = fs::create_dir_all(&out_dir);
    let air_path = format!("{}/ggml-metal.air", out_dir);
    let metallib_path = format!("{}/ggml-default.metallib", out_dir);

    if !precompile_enabled {
        let _ = fs::write(&metallib_path, []);
        println!(
            "cargo:rustc-env=MAKEPAD_VOICE_GGML_METALLIB={}",
            metallib_path
        );
        return;
    }

    let metal_status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "metal",
            "-O3",
            "-c",
            &metal_src,
            "-I",
            &ggml_src_dir,
            "-I",
            &ggml_metal_dir,
            "-o",
            &air_path,
        ])
        .status();

    let ok = metal_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!("cargo:warning=failed to compile ggml-metal.metal to AIR; runtime source compile will be used");
        let _ = fs::write(&metallib_path, []);
        println!(
            "cargo:rustc-env=MAKEPAD_VOICE_GGML_METALLIB={}",
            metallib_path
        );
        return;
    }

    let metallib_status = Command::new("xcrun")
        .args([
            "--sdk",
            "macosx",
            "metallib",
            &air_path,
            "-o",
            &metallib_path,
        ])
        .status();

    let ok = metallib_status.as_ref().is_ok_and(|s| s.success());
    if !ok {
        println!("cargo:warning=failed to build ggml default metallib; runtime source compile will be used");
        let _ = fs::write(&metallib_path, []);
    }

    println!(
        "cargo:rustc-env=MAKEPAD_VOICE_GGML_METALLIB={}",
        metallib_path
    );
}

fn build_speech_bridge(target_os: &str) {
    let out_dir = env::var("OUT_DIR").unwrap();
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let swift_src = format!("{}/swift/speech_bridge.swift", manifest_dir);
    let swift_module_cache = format!("{}/swift_module_cache", out_dir);

    println!("cargo:rerun-if-changed=swift/speech_bridge.swift");
    let _ = fs::create_dir_all(&swift_module_cache);

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

    if target_os == "macos" {
        fix_swift_rpath_tbds(&out_dir);
    }

    println!("cargo:rustc-link-search=native={}", out_dir);
    println!("cargo:rustc-link-lib=static=speech_bridge");

    println!("cargo:rustc-link-lib=framework=Speech");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AVFoundation");
    println!("cargo:rustc-link-lib=framework=CoreMedia");

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
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())?;
    Some((swift_target, sdk_path))
}

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

    println!("cargo:rustc-link-search=native={}", override_dir);
}

fn strip_ld_previous_rpath(content: &str) -> String {
    let mut result = content.to_string();

    while let Some(start) = result.find("'$ld$previous$@rpath/") {
        if let Some(end_quote_offset) = result[start + 1..].find('\'') {
            let end = start + 1 + end_quote_offset + 1;
            let rest = &result[end..];
            let trimmed =
                rest.trim_start_matches(|c: char| c == ',' || c == ' ' || c == '\n' || c == '\r');
            let skip = rest.len() - trimmed.len();
            result = format!("{}{}", &result[..start], &result[end + skip..]);
        } else {
            break;
        }
    }

    result
}
