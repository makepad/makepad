use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn media_libs_root() -> Option<PathBuf> {
    if let Ok(root) = env::var("MAKEPAD_MEDIA_LIBS") {
        let path = PathBuf::from(root);
        if path.is_dir() {
            return Some(path);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let sibling = manifest_dir.join("..").join("..").join("makepad-media-libs");
    if sibling.is_dir() {
        return Some(sibling);
    }

    None
}

fn latest_versioned_subdir(parent: &Path) -> Option<PathBuf> {
    if !parent.is_dir() {
        return None;
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(parent)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    candidates.sort();
    candidates.pop()
}

fn detect_android_ndk_root() -> Option<PathBuf> {
    for key in [
        "ANDROID_NDK_HOME",
        "ANDROID_NDK_ROOT",
        "ANDROID_NDK",
        "NDK_HOME",
        "NDK_ROOT",
    ] {
        if let Ok(val) = env::var(key) {
            let p = PathBuf::from(val);
            if p.is_dir() {
                return Some(p);
            }
        }
    }

    for key in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Ok(val) = env::var(key) {
            let ndk_parent = PathBuf::from(val).join("ndk");
            if let Some(p) = latest_versioned_subdir(&ndk_parent) {
                return Some(p);
            }
        }
    }

    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let manifest_dir = PathBuf::from(manifest_dir);
        let roots = [manifest_dir.join("..")];
        for root in roots {
            for host in ["linux_x64", "macos_x64", "macos_aarch64", "windows_x64"] {
                let ndk_parent = root
                    .join("tools")
                    .join("cargo_makepad")
                    .join(format!("android_33_{host}"))
                    .join("ndk");
                if let Some(p) = latest_versioned_subdir(&ndk_parent) {
                    return Some(p);
                }
            }
        }
    }

    if let Ok(home) = env::var("HOME") {
        let ndk_parent = Path::new(&home).join("Android-Sdk").join("ndk");
        if let Some(p) = latest_versioned_subdir(&ndk_parent) {
            return Some(p);
        }
    }

    None
}

fn configure_android_cc(build: &mut cc::Build, target_arch: &str) {
    let Some(ndk_root) = detect_android_ndk_root() else {
        return;
    };

    let host_tags: &[&str] = if cfg!(target_os = "linux") {
        &["linux-x86_64"]
    } else if cfg!(target_os = "macos") {
        &["darwin-x86_64", "darwin-aarch64"]
    } else if cfg!(target_os = "windows") {
        &["windows-x86_64"]
    } else {
        return;
    };

    let Some(llvm_root) = host_tags
        .iter()
        .map(|tag| {
            ndk_root
                .join("toolchains")
                .join("llvm")
                .join("prebuilt")
                .join(tag)
        })
        .find(|p| p.is_dir())
    else {
        return;
    };
    let sysroot = llvm_root.join("sysroot");
    if sysroot.is_dir() {
        build.flag(&format!("--sysroot={}", sysroot.display()));
    }

    let api = env::var("ANDROID_API_LEVEL")
        .ok()
        .or_else(|| env::var("ANDROID_PLATFORM").ok())
        .and_then(|s| s.trim_start_matches("android-").parse::<u32>().ok())
        .unwrap_or(33);

    let clang_bin = llvm_root.join("bin");
    let tool = match target_arch {
        "aarch64" => format!("aarch64-linux-android{api}-clang"),
        "arm" => format!("armv7a-linux-androideabi{api}-clang"),
        "x86_64" => format!("x86_64-linux-android{api}-clang"),
        "x86" => format!("i686-linux-android{api}-clang"),
        _ => return,
    };
    let tool_path = clang_bin.join(tool);
    if tool_path.is_file() {
        build.compiler(tool_path);
    }
}

fn svt_machine_for_arch(target_arch: &str) -> Option<u16> {
    match target_arch {
        "x86_64" => Some(62),
        "x86" => Some(3),
        "aarch64" => Some(183),
        "arm" => Some(40),
        _ => None,
    }
}

fn svt_archive_matches_target(lib_path: &Path, target_arch: &str) -> bool {
    let Some(expected_machine) = svt_machine_for_arch(target_arch) else {
        return false;
    };

    let members = match Command::new("ar").arg("t").arg(lib_path).output() {
        Ok(out) if out.status.success() => out.stdout,
        _ => return false,
    };
    let members = match String::from_utf8(members) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let Some(first_member) = members.lines().map(str::trim).find(|line| !line.is_empty()) else {
        return false;
    };

    let object = match Command::new("ar")
        .arg("p")
        .arg(lib_path)
        .arg(first_member)
        .output()
    {
        Ok(out) if out.status.success() => out.stdout,
        _ => return false,
    };

    if object.len() < 20 || &object[0..4] != b"\x7FELF" {
        return false;
    }

    let machine = u16::from_le_bytes([object[18], object[19]]);
    machine == expected_machine
}

fn apply_dav1d_defines(build: &mut cc::Build, target_arch: &str, target_os: &str) {
    let (arch_x86, arch_x86_64, arch_aarch64, arch_arm) = match target_arch {
        "x86_64" => (true, true, false, false),
        "x86" => (true, false, false, false),
        "aarch64" => (false, false, true, false),
        "arm" => (false, false, false, true),
        _ => (false, false, false, false),
    };

    build
        .define("HAVE_ASM", "0")
        .define("CONFIG_8BPC", "1")
        .define("CONFIG_16BPC", "1")
        .define("CONFIG_LOG", "0")
        .define("CONFIG_MACOS_KPERF", "0")
        .define("TRIM_DSP_FUNCTIONS", "1")
        .define("ENDIANNESS_BIG", "0")
        .define("HAVE_C11_GENERIC", "1")
        .define("ARCH_X86", if arch_x86 { "1" } else { "0" })
        .define("ARCH_X86_64", if arch_x86_64 { "1" } else { "0" })
        .define(
            "ARCH_X86_32",
            if arch_x86 && !arch_x86_64 { "1" } else { "0" },
        )
        .define("ARCH_AARCH64", if arch_aarch64 { "1" } else { "0" })
        .define("ARCH_ARM", if arch_arm { "1" } else { "0" })
        .define("ARCH_PPC64LE", "0")
        .define("ARCH_RISCV", "0")
        .define("ARCH_RV32", "0")
        .define("ARCH_RV64", "0")
        .define("ARCH_LOONGARCH", "0")
        .define("ARCH_LOONGARCH32", "0")
        .define("ARCH_LOONGARCH64", "0");

    match target_os {
        "linux" | "android" | "macos" | "ios" | "tvos" | "freebsd" => {
            build
                .define("HAVE_POSIX_MEMALIGN", "1")
                .define("HAVE_DLSYM", "1")
                .define("HAVE_UNISTD_H", "1")
                .define("HAVE_CLOCK_GETTIME", "1")
                .define("HAVE_SYS_TYPES_H", "1")
                .define("HAVE_IO_H", "0")
                .define("HAVE_PTHREAD_NP_H", "0")
                .define("HAVE_PTHREAD_GETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETNAME_NP", "0")
                .define("HAVE_PTHREAD_SET_NAME_NP", "0")
                .define("HAVE_ELF_AUX_INFO", "0");

            if target_os == "linux" {
                build.define("HAVE_GETAUXVAL", "1");
                build.define("HAVE_MEMALIGN", "1");
                build.define("HAVE_ALIGNED_ALLOC", "1");
            } else if target_os == "android" {
                build.define("HAVE_GETAUXVAL", "1");
                build.define("HAVE_MEMALIGN", "0");
                build.define("HAVE_ALIGNED_ALLOC", "0");
            } else {
                build.define("HAVE_GETAUXVAL", "0");
                build.define("HAVE_MEMALIGN", "0");
                build.define("HAVE_ALIGNED_ALLOC", "0");
            }
        }
        "windows" => {
            build
                .define("HAVE_POSIX_MEMALIGN", "0")
                .define("HAVE_MEMALIGN", "0")
                .define("HAVE_ALIGNED_ALLOC", "0")
                .define("HAVE_DLSYM", "0")
                .define("HAVE_UNISTD_H", "0")
                .define("HAVE_IO_H", "1")
                .define("HAVE_CLOCK_GETTIME", "0")
                .define("HAVE_SYS_TYPES_H", "1")
                .define("HAVE_PTHREAD_NP_H", "0")
                .define("HAVE_GETAUXVAL", "0")
                .define("HAVE_ELF_AUX_INFO", "0")
                .define("HAVE_PTHREAD_GETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETNAME_NP", "0")
                .define("HAVE_PTHREAD_SET_NAME_NP", "0")
                .define("_WIN32_WINNT", "0x0601")
                .define("UNICODE", "1")
                .define("_UNICODE", "1");
        }
        _ => {
            build
                .define("HAVE_POSIX_MEMALIGN", "0")
                .define("HAVE_MEMALIGN", "0")
                .define("HAVE_ALIGNED_ALLOC", "0")
                .define("HAVE_DLSYM", "0")
                .define("HAVE_UNISTD_H", "0")
                .define("HAVE_IO_H", "0")
                .define("HAVE_CLOCK_GETTIME", "0")
                .define("HAVE_SYS_TYPES_H", "0")
                .define("HAVE_PTHREAD_NP_H", "0")
                .define("HAVE_GETAUXVAL", "0")
                .define("HAVE_ELF_AUX_INFO", "0")
                .define("HAVE_PTHREAD_GETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETAFFINITY_NP", "0")
                .define("HAVE_PTHREAD_SETNAME_NP", "0")
                .define("HAVE_PTHREAD_SET_NAME_NP", "0");
        }
    }
}

fn build_dav1d() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let Some(media_libs_root) = media_libs_root() else {
        return;
    };
    let dav1d_dir = media_libs_root.join("dav1d");
    let src_dir = dav1d_dir.join("src");
    let include_dir = dav1d_dir.join("include");

    let core_sources: Vec<&str> = vec![
        "cdf.c",
        "cpu.c",
        "ctx.c",
        "data.c",
        "decode.c",
        "dequant_tables.c",
        "getbits.c",
        "intra_edge.c",
        "itx_1d.c",
        "lf_mask.c",
        "lib.c",
        "log.c",
        "mem.c",
        "msac.c",
        "obu.c",
        "pal.c",
        "picture.c",
        "qm.c",
        "ref.c",
        "refmvs.c",
        "scan.c",
        "tables.c",
        "thread_task.c",
        "warpmv.c",
        "wedge.c",
    ];

    let tmpl_sources: Vec<&str> = vec![
        "cdef_apply_tmpl.c",
        "cdef_tmpl.c",
        "fg_apply_tmpl.c",
        "filmgrain_tmpl.c",
        "ipred_prepare_tmpl.c",
        "ipred_tmpl.c",
        "itx_tmpl.c",
        "lf_apply_tmpl.c",
        "loopfilter_tmpl.c",
        "looprestoration_tmpl.c",
        "lr_apply_tmpl.c",
        "mc_tmpl.c",
        "recon_tmpl.c",
    ];

    let mut build = cc::Build::new();
    build
        .warnings(false)
        .include(&include_dir)
        .include(&dav1d_dir)
        .include(&src_dir)
        .define("HAVE_ASM", "0")
        .define("CONFIG_8BPC", "1")
        .define("CONFIG_16BPC", "1")
        .define("CONFIG_LOG", "0")
        .define("CONFIG_MACOS_KPERF", "0")
        .define("TRIM_DSP_FUNCTIONS", "1")
        .define("ENDIANNESS_BIG", "0")
        .define("HAVE_C11_GENERIC", "1");

    apply_dav1d_defines(&mut build, &target_arch, &target_os);

    for src in &core_sources {
        build.file(src_dir.join(src));
    }

    if target_os == "android" {
        configure_android_cc(&mut build, &target_arch);
    }

    build.compile("dav1d_core");

    let mut build_8 = cc::Build::new();
    build_8
        .warnings(false)
        .include(&include_dir)
        .include(&dav1d_dir)
        .include(&src_dir)
        .define("BITDEPTH", "8");
    apply_dav1d_defines(&mut build_8, &target_arch, &target_os);
    for src in &tmpl_sources {
        build_8.file(src_dir.join(src));
    }
    if target_os == "android" {
        configure_android_cc(&mut build_8, &target_arch);
    }
    build_8.compile("dav1d_tmpl_8");

    let mut build_16 = cc::Build::new();
    build_16
        .warnings(false)
        .include(&include_dir)
        .include(&dav1d_dir)
        .include(&src_dir)
        .define("BITDEPTH", "16");
    apply_dav1d_defines(&mut build_16, &target_arch, &target_os);
    for src in &tmpl_sources {
        build_16.file(src_dir.join(src));
    }
    if target_os == "android" {
        configure_android_cc(&mut build_16, &target_arch);
    }
    build_16.compile("dav1d_tmpl_16");

    match target_os.as_str() {
        "linux" | "macos" | "ios" | "tvos" | "freebsd" => {
            println!("cargo:rustc-link-lib=pthread");
        }
        _ => {}
    }
    if target_os == "linux" || target_os == "android" {
        println!("cargo:rustc-link-lib=dl");
    }

    println!("cargo:rustc-cfg=has_dav1d");
}

fn build_svt_av1() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();
    if target_os != "linux" && target_os != "android" && target_os != "ios" {
        return;
    }

    let Some(media_libs_root) = media_libs_root() else {
        return;
    };
    let svt_root = media_libs_root.join("svt-av1");
    if !svt_root.exists() {
        return;
    }

    let mut lib_dir = svt_root.join("Bin").join("Release");
    let mut lib_path = if target_os == "linux" {
        let candidate = lib_dir.join("libSvtAv1Enc.a");
        if candidate.exists() && svt_archive_matches_target(&candidate, &target_arch) {
            candidate
        } else {
            if candidate.exists() {
                println!(
                    "cargo:warning=Skipping bundled libSvtAv1Enc.a because it does not match target arch {}",
                    target_arch
                );
            }
            PathBuf::new()
        }
    } else {
        PathBuf::new()
    };

    if target_os == "android" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("svt-av1-android");
        let _ = std::fs::create_dir_all(&out_dir);

        let abi = match target_arch.as_str() {
            "aarch64" => "arm64-v8a",
            "arm" => "armeabi-v7a",
            "x86_64" => "x86_64",
            "x86" => "x86",
            _ => return,
        };

        let prebuilt_candidates = [
            svt_root
                .join("Bin")
                .join("Android")
                .join(abi)
                .join("libSvtAv1Enc.a"),
            svt_root
                .join("Bin")
                .join("Release")
                .join(abi)
                .join("libSvtAv1Enc.a"),
            svt_root.join("Bin").join("Release").join("libSvtAv1Enc.a"),
        ];
        if let Some(found) = prebuilt_candidates
            .iter()
            .find(|p| p.exists() && svt_archive_matches_target(p, &target_arch))
        {
            lib_path = found.clone();
            lib_dir = found.parent().unwrap().to_path_buf();
        }

        if !lib_path.exists() {
            let Some(ndk_root) = detect_android_ndk_root() else {
                return;
            };

            let api = env::var("ANDROID_API_LEVEL")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(29);

            let cmake_toolchain = ndk_root
                .join("build")
                .join("cmake")
                .join("android.toolchain.cmake");

            let configure = Command::new("cmake")
                .arg("-S")
                .arg(&svt_root)
                .arg("-B")
                .arg(&out_dir)
                .arg("-DCMAKE_BUILD_TYPE=Release")
                .arg("-DBUILD_SHARED_LIBS=OFF")
                .arg("-DSVT_AV1_LTO=OFF")
                .arg("-DCMAKE_SYSTEM_NAME=Android")
                .arg(format!("-DANDROID_ABI={abi}"))
                .arg(format!("-DANDROID_PLATFORM=android-{api}"))
                .arg(format!("-DCMAKE_ANDROID_NDK={}", ndk_root.display()))
                .arg(format!("-DCMAKE_TOOLCHAIN_FILE={}", cmake_toolchain.display()))
                .status();

            if !matches!(configure, Ok(s) if s.success()) {
                return;
            }

            let build = Command::new("cmake")
                .arg("--build")
                .arg(&out_dir)
                .arg("--config")
                .arg("Release")
                .arg("--target")
                .arg("SvtAv1Enc")
                .status();

            if !matches!(build, Ok(s) if s.success()) {
                return;
            }

            let candidates = [
                out_dir.join("Bin").join("Release").join("libSvtAv1Enc.a"),
                out_dir.join("Release").join("libSvtAv1Enc.a"),
                out_dir.join("libSvtAv1Enc.a"),
                svt_root.join("Bin").join("Release").join("libSvtAv1Enc.a"),
            ];
            if let Some(found) = candidates
                .iter()
                .find(|p| p.exists() && svt_archive_matches_target(p, &target_arch))
            {
                lib_path = found.clone();
                lib_dir = found.parent().unwrap().to_path_buf();
            }
        }

        if !lib_path.exists() {
            return;
        }
    }

    if target_os == "ios" {
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("svt-av1-ios");
        let _ = std::fs::create_dir_all(&out_dir);

        let arch = match target_arch.as_str() {
            "aarch64" => "arm64",
            "x86_64" => "x86_64",
            _ => return,
        };

        let is_sim = target.contains("apple-ios-sim") || target.starts_with("x86_64-apple-ios");
        let sdk = if is_sim {
            "iphonesimulator"
        } else {
            "iphoneos"
        };

        let prebuilt_candidates = [
            svt_root
                .join("Bin")
                .join("iOS")
                .join(sdk)
                .join(arch)
                .join("libSvtAv1Enc.a"),
            svt_root
                .join("Bin")
                .join("iOS")
                .join(arch)
                .join("libSvtAv1Enc.a"),
            svt_root
                .join("Bin")
                .join("Release")
                .join(sdk)
                .join(arch)
                .join("libSvtAv1Enc.a"),
            svt_root.join("Bin").join("Release").join("libSvtAv1Enc.a"),
        ];

        if let Some(found) = prebuilt_candidates
            .iter()
            .find(|p| p.exists() && svt_archive_matches_target(p, &target_arch))
        {
            lib_path = found.clone();
            lib_dir = found.parent().unwrap().to_path_buf();
        }

        if !lib_path.exists() {
            let mut configure = Command::new("cmake");
            configure
                .arg("-S")
                .arg(&svt_root)
                .arg("-B")
                .arg(&out_dir)
                .arg("-G")
                .arg("Xcode")
                .arg("-DCMAKE_SYSTEM_NAME=iOS")
                .arg(format!("-DCMAKE_OSX_SYSROOT={sdk}"))
                .arg(format!("-DCMAKE_OSX_ARCHITECTURES={arch}"))
                .arg("-DCMAKE_BUILD_TYPE=Release")
                .arg("-DBUILD_SHARED_LIBS=OFF")
                .arg("-DBUILD_APPS=OFF")
                .arg("-DBUILD_TESTING=OFF")
                .arg("-DSVT_AV1_LTO=OFF")
                .arg("-DCMAKE_XCODE_ATTRIBUTE_CODE_SIGNING_ALLOWED=NO");

            if let Ok(min_version) = env::var("IPHONEOS_DEPLOYMENT_TARGET") {
                if !min_version.trim().is_empty() {
                    configure.arg(format!("-DCMAKE_OSX_DEPLOYMENT_TARGET={}", min_version));
                }
            }

            let configure_status = configure.status();
            if !matches!(configure_status, Ok(s) if s.success()) {
                return;
            }

            let build = Command::new("cmake")
                .arg("--build")
                .arg(&out_dir)
                .arg("--config")
                .arg("Release")
                .arg("--target")
                .arg("SvtAv1Enc")
                .status();

            if !matches!(build, Ok(s) if s.success()) {
                return;
            }

            let candidates = [
                svt_root.join("Bin").join("Release").join("libSvtAv1Enc.a"),
                out_dir.join(format!("Release-{sdk}")).join("libSvtAv1Enc.a"),
                out_dir.join("Release").join("libSvtAv1Enc.a"),
                out_dir.join("Bin").join("Release").join("libSvtAv1Enc.a"),
                out_dir.join("libSvtAv1Enc.a"),
            ];
            if let Some(found) = candidates
                .iter()
                .find(|p| p.exists() && svt_archive_matches_target(p, &target_arch))
            {
                lib_path = found.clone();
                lib_dir = found.parent().unwrap().to_path_buf();
            }
        }

        if !lib_path.exists() {
            return;
        }
    }

    if !lib_path.exists() {
        return;
    }

    let mut wrapper = cc::Build::new();
    wrapper
        .warnings(false)
        .include(svt_root.join("Source").join("API"))
        .file("src/svt_av1_wrapper.c");
    if target_os == "android" {
        configure_android_cc(&mut wrapper, &target_arch);
    }
    wrapper.compile("mp_svt_av1_wrapper");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=SvtAv1Enc");
    println!("cargo:rustc-cfg=has_svt_av1");
    println!("cargo:rerun-if-changed=src/svt_av1_wrapper.c");
    println!(
        "cargo:rerun-if-changed={}",
        svt_root.join("Source/API/EbSvtAv1Enc.h").display()
    );
    println!("cargo:rerun-if-changed={}", lib_path.display());
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(has_svt_av1,has_dav1d)");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target = env::var("TARGET").unwrap_or_default();

    let dav1d_enabled = env::var_os("CARGO_FEATURE_DAV1D").is_some();
    let svt_av1_enabled = env::var_os("CARGO_FEATURE_SVT_AV1").is_some();

    if target_os != "unknown" && !target.contains("wasm") && !target.contains("ohos") {
        if dav1d_enabled {
            build_dav1d();
        }
        if svt_av1_enabled {
            build_svt_av1();
        }
    }
}
