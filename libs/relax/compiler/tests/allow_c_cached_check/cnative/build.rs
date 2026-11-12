fn main() {
    cc::Build::new().file("csrc/cnative.c").compile("cnative");
    println!("cargo:rerun-if-changed=csrc/cnative.c");
    // A -sys crate's generated bindings: an OUT_DIR file the crate includes, so a
    // compile without the script's outputs loses a type from every signature that names it.
    let out = std::env::var("OUT_DIR").unwrap();
    // MARK: what this script wrote (verify.py moves it to tell two runs of the script apart)
    std::fs::write(std::path::Path::new(&out).join("gen.rs"), "pub struct Generated(pub i32);\npub const MARK: i32 = 1;\n").unwrap();
}
