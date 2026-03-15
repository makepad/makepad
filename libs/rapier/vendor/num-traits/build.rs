fn main() {
    // Our toolchain is new enough to always have `f64::total_cmp` (stable since 1.62).
    // Keep the same cfg flag that upstream probes via autocfg, but without the dependency.
    println!("cargo:rustc-cfg=has_total_cmp");
    println!("cargo:rerun-if-changed=build.rs");
}
