// A build-dependency's own script: the forced `[env]` HOME reaches it too.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(gen_home_configured)");
    if std::env::var_os("HOME").is_some_and(|h| std::path::Path::new(&h).ends_with("configured-home")) {
        println!("cargo:rustc-cfg=gen_home_configured");
    }
}
