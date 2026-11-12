// cargo's build-script environment holds its package's variables as its crates get them
fn main() {
    for k in ["CARGO_MANIFEST_DIR", "CARGO_MANIFEST_PATH", "CARGO_PKG_NAME", "CARGO_PKG_VERSION_MAJOR", "CARGO_PKG_VERSION_PRE", "CARGO_PKG_AUTHORS"] {
        let v = std::env::var(k).unwrap_or_else(|_| panic!("the build script's environment has no {k}"));
        println!("cargo:rustc-env=RUN_ENV_SCRIPT_{k}={v}");
    }
    println!("cargo:rustc-env=RUN_ENV_FROM_SCRIPT=script");
}
