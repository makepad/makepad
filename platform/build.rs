use std::env;
use std::fs::File;
use std::path::Path;
use std::io::prelude::*;

fn main() {
    // write a path to makepad platform into our output dir
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir).parent().unwrap().parent().unwrap().parent().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-platform.path")).unwrap();
    file.write_all(&format!("{}", cwd.display()).as_bytes()).unwrap();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let target = env::var("TARGET").unwrap();
    println!("cargo:rustc-check-cfg=cfg(apple_bundle,apple_sim,lines,linux_direct,no_android_choreographer,use_unstable_unix_socket_ancillary_data_2021)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    println!("cargo:rerun-if-env-changed=MAKEPAD_PACKAGE_DIR");
    if let Ok(configs) = env::var("MAKEPAD"){
        for config in configs.split('+'){
            match config{
                "lines"=>println!("cargo:rustc-cfg=lines"), 
                "linux_direct"=>println!("cargo:rustc-cfg=linux_direct"), 
                "no_android_choreographer"=>println!("cargo:rustc-cfg=no_android_choreographer"), 
                "apple_bundle"=>println!("cargo:rustc-cfg=apple_bundle"), 
                _=>{}
            }
        }
    }
    
    match target_os.as_str(){
        "macos"=>{
            use std::process::Command;
            let out_dir = env::var("OUT_DIR").unwrap();
            if !Command::new("clang").args(&["src/os/apple/metal_xpc.m", "-c", "-o"])
                .arg(&format!("{}/metal_xpc.o", out_dir))
                .status().unwrap().success() {
                panic!("CLANG FAILED");
            };
            
            if !Command::new("ar").args(&["crus", "libmetal_xpc.a", "metal_xpc.o"])
                .current_dir(&Path::new(&out_dir))
                .status().unwrap().success() {
                panic!("AR FAILED"); 
            };
            
            println!("cargo:rustc-link-search=native={}", out_dir);
            println!("cargo:rustc-link-lib=static=metal_xpc");
            println!("cargo:rerun-if-changed=src/os/apple/metal_xpc.m");
        }
        "ios"=>{
            if target == "aarch64-apple-ios-sim"{
                println!("cargo:rustc-cfg=apple_sim"); 
                //println!("cargo:rustc-cfg=apple_bundle"); 
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
        }
        "tvos"=>{
            if target == "aarch64-apple-tvos-sim"{
                println!("cargo:rustc-cfg=apple_sim"); 
                //println!("cargo:rustc-cfg=apple_bundle"); 
            }
            println!("cargo:rustc-link-lib=framework=MetalKit");
        }
        _=>()
    }
}