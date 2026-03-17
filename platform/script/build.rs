use std::env;

fn main() {
    println!("cargo:rustc-check-cfg=cfg(use_vulkan)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");

    if let Ok(configs) = env::var("MAKEPAD") {
        for config in configs.split(['+', ',']) {
            if matches!(config, "vulkan" | "use_vulkan") {
                println!("cargo:rustc-cfg=use_vulkan");
            }
        }
    }
}
