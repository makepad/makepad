fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-env=HELPER_A={}", script_nest_helper_b::scan());
}
