fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("TARGET").is_ok_and(|v| v == "x86_64-pc-windows-msvc") {
        for arg in ["/entry:mainCRTStartup", "/subsystem:console", "/nodefaultlib"] {
            println!("cargo:rustc-link-arg-bin=makepad-xr-proton={arg}");
        }
    }
}
