fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let plist = manifest.join("examples/Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    println!("cargo:rustc-link-arg-examples=-Wl,-sectcreate,__TEXT,__info_plist,{}", plist.display());
}
