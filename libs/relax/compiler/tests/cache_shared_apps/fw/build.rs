fn main() {
    println!("cargo:rerun-if-env-changed=SC_FLAVOR");
    if std::env::var("SC_FLAVOR").map_or(false, |v| v == "b") {
        println!("cargo:rustc-cfg=flavor_b");
    }
}
