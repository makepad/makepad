fn main() {
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let plist = manifest.join("examples/Info.plist");
    println!("cargo:rerun-if-changed={}", plist.display());
    // The example's Info.plist (NSAudioCaptureUsageDescription) is a macOS
    // linker section; other linkers do not take the flag.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-examples=-Wl,-sectcreate,__TEXT,__info_plist,{}", plist.display());
    }
}
