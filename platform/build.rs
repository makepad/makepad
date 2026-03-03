use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

/// Build vendored dav1d C library (scalar-only, no ASM).
fn build_dav1d() {
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let dav1d_dir = Path::new("src/os/vendor/dav1d");
    let src_dir = dav1d_dir.join("src");
    let include_dir = dav1d_dir.join("include");

    // Core C sources (non-template)
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

    // Template sources compiled once per bitdepth
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

    // Architecture defines
    let (arch_x86, arch_x86_64, arch_aarch64, arch_arm) = match target_arch.as_str() {
        "x86_64" => (true, true, false, false),
        "x86" => (true, false, false, false),
        "aarch64" => (false, false, true, false),
        "arm" => (false, false, false, true),
        _ => (false, false, false, false),
    };

    let mut build = cc::Build::new();
    build
        .warnings(false)
        .include(&include_dir)
        .include(dav1d_dir) // for config.h
        .include(&src_dir) // for internal headers
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

    // POSIX features
    match target_os.as_str() {
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

    // Add core sources
    for src in &core_sources {
        build.file(src_dir.join(src));
    }

    build.compile("dav1d_core");

    // Compile template sources for 8-bit
    let mut build_8 = cc::Build::new();
    build_8
        .warnings(false)
        .include(&include_dir)
        .include(dav1d_dir)
        .include(&src_dir)
        .define("BITDEPTH", "8");
    // Copy all defines from main build
    apply_dav1d_defines(&mut build_8, &target_arch, &target_os);
    for src in &tmpl_sources {
        build_8.file(src_dir.join(src));
    }
    build_8.compile("dav1d_tmpl_8");

    // Compile template sources for 16-bit
    let mut build_16 = cc::Build::new();
    build_16
        .warnings(false)
        .include(&include_dir)
        .include(dav1d_dir)
        .include(&src_dir)
        .define("BITDEPTH", "16");
    apply_dav1d_defines(&mut build_16, &target_arch, &target_os);
    for src in &tmpl_sources {
        build_16.file(src_dir.join(src));
    }
    build_16.compile("dav1d_tmpl_16");

    // Link pthread on Unix
    match target_os.as_str() {
        "linux" | "macos" | "ios" | "tvos" | "freebsd" => {
            println!("cargo:rustc-link-lib=pthread");
        }
        _ => {}
    }
    if target_os == "linux" || target_os == "android" {
        println!("cargo:rustc-link-lib=dl");
    }
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

fn main() {
    // write a path to makepad platform into our output dir
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-platform.path")).unwrap();
    file.write_all(&format!("{}", cwd.display()).as_bytes())
        .unwrap();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target = env::var("TARGET").unwrap();

    let icon_vars = [
        "MAKEPAD_APP_ICON_32",
        "MAKEPAD_APP_ICON_64",
        "MAKEPAD_APP_ICON_128",
        "MAKEPAD_APP_ICON_256",
        "MAKEPAD_APP_ICON_512",
        "MAKEPAD_APP_ICON_1024",
        "MAKEPAD_APP_ICON_ICO",
    ];
    let icons = icon_vars.map(|var| env::var(var).ok());
    for path in icons.iter().flatten() {
        println!("cargo:rerun-if-changed={}", path);
    }
    let include_or_empty = |path: &Option<String>| {
        path.as_ref()
            .map(|p| format!("include_bytes!(r#\"{}\"#)", p))
            .unwrap_or_else(|| "&[]".to_string())
    };
    let icon_gen = format!(
        "pub static CUSTOM_ICON_PNG_32: &'static [u8] = {};\n\
pub static CUSTOM_ICON_PNG_64: &'static [u8] = {};\n\
pub static CUSTOM_ICON_PNG_128: &'static [u8] = {};\n\
pub static CUSTOM_ICON_PNG_256: &'static [u8] = {};\n\
pub static CUSTOM_ICON_PNG_512: &'static [u8] = {};\n\
pub static CUSTOM_ICON_PNG_1024: &'static [u8] = {};\n\
#[allow(dead_code)]\n\
pub static CUSTOM_ICON_ICO: &'static [u8] = {};\n",
        include_or_empty(&icons[0]),
        include_or_empty(&icons[1]),
        include_or_empty(&icons[2]),
        include_or_empty(&icons[3]),
        include_or_empty(&icons[4]),
        include_or_empty(&icons[5]),
        include_or_empty(&icons[6]),
    );
    std::fs::write(Path::new(&out_dir).join("app_icon_gen.rs"), icon_gen).unwrap();
    println!("cargo:rustc-check-cfg=cfg(apple_bundle,apple_sim,lines,use_gles_3,use_vulkan,linux_direct,quest,no_android_choreographer,ohos_sim,headless,use_unstable_unix_socket_ancillary_data_2021)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=MAKEPAD_PACKAGE_DIR");
    for var in icon_vars {
        println!("cargo:rerun-if-env-changed={var}");
    }

    if let Ok(configs) = env::var("MAKEPAD") {
        for config in configs.split(['+', ',']) {
            match config {
                "lines" => println!("cargo:rustc-cfg=lines"),
                "linux_direct" => println!("cargo:rustc-cfg=linux_direct"),
                "no_android_choreographer" => println!("cargo:rustc-cfg=no_android_choreographer"),
                "quest" => {
                    println!("cargo:rustc-cfg=quest");
                    println!("cargo:rustc-cfg=use_gles_3");
                }
                "apple_bundle" => println!("cargo:rustc-cfg=apple_bundle"),
                "ohos_sim" => println!("cargo:rustc-cfg=ohos_sim"),
                "headless" => println!("cargo:rustc-cfg=headless"),
                "use_gles_3" => println!("cargo:rustc-cfg=use_gles_3"),
                "vulkan" | "use_vulkan" => println!("cargo:rustc-cfg=use_vulkan"),
                _ => {}
            }
        }
    }

    // Build vendored dav1d (not for wasm or ohos)
    if target_os != "unknown" && !target.contains("wasm") && !target.contains("ohos") {
        build_dav1d();
    }

    match target_os.as_str() {
        "macos" => {
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "ios" => {
            if target == "aarch64-apple-ios-sim" {
                println!("cargo:rustc-cfg=apple_sim");
                //println!("cargo:rustc-cfg=apple_bundle");
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "tvos" => {
            if target == "aarch64-apple-tvos-sim" {
                println!("cargo:rustc-cfg=apple_sim");
                //println!("cargo:rustc-cfg=apple_bundle");
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
            println!("cargo:rustc-link-lib=framework=GameController");
        }
        "linux" => {
            println!("cargo:rustc-cfg=use_gles_3");
            println!("cargo:rustc-link-lib=xkbcommon");
        }
        "android" => {
            println!("cargo:rustc-cfg=use_gles_3");
        }
        _ => (),
    }
}
