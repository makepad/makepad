fn main() {
    println!("cargo:rustc-check-cfg=cfg(headless)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if std::env::var("MAKEPAD").unwrap_or_default().split(',').any(|v| v == "headless") {
        println!("cargo:rustc-cfg=headless");
    }
}
