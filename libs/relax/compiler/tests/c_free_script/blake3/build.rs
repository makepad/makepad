// As blake3's script: with `pure` it builds the portable implementation and no C.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_FEATURE_PURE").is_none() {
        cc::compile("c/blake3_avx512.c");
    }
    println!("cargo:rustc-env=BLAKE3_IMPL=portable");
}
