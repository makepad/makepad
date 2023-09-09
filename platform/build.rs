use std::env;
fn main() {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if let Ok(configs) = env::var("MAKEPAD"){
        for config in configs.split('+'){
            match config{
                "lines"=>println!("cargo:rustc-cfg=lines"), 
                "linux_direct"=>println!("cargo:rustc-cfg=linux_direct"), 
                _=>()
            }
        }
    }
    
    match target_os.as_str(){
        "macos"=>{
            use std::process::Command;
            use std::path::Path;
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
            println!("cargo:rustc-link-lib=framework=MetalKit");
        }
        _=>()
    }
}