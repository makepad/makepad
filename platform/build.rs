fn main() {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        use std::env;
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
    #[cfg(target_arch = "wasm32")]
    {
    }
    #[cfg(any(target_os = "linux", target_os="windows"))]
    {
        //panic!("Linux and windows support coming soon")
    }
}