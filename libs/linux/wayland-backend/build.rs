fn main() {
    println!("cargo:rustc-check-cfg=cfg(coverage)");
    println!("cargo:rustc-check-cfg=cfg(unstable_coverage)");
    println!("cargo:rerun-if-changed=build.rs");
}
