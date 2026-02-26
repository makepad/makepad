use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

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
