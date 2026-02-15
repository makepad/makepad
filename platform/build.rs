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
    println!("cargo:rustc-check-cfg=cfg(apple_bundle,apple_sim,lines,use_gles_3,linux_direct,quest,no_android_choreographer,ohos_sim,headless,use_unstable_unix_socket_ancillary_data_2021)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=MAKEPAD_PACKAGE_DIR");

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
