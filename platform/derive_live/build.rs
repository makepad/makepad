use std::env;
fn main() {
    println!("cargo:rustc-check-cfg=cfg(lines)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if let Ok(configs) = env::var("MAKEPAD"){
        for config in configs.split('+'){
            if config == "lines" {
                println!("cargo:rustc-cfg=lines")
            }
        }
    }
}
