fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=APP_SCAN={}", script_nest_helper_a::scan());
}
