fn main() {
    println!("cargo:rustc-check-cfg=cfg(gpusim)");
    println!("cargo:rerun-if-env-changed=MAKEPAD");
    if std::env::var("MAKEPAD").unwrap_or_default().split(',').any(|v| v == "gpusim") {
        println!("cargo:rustc-cfg=gpusim");
    }
}
