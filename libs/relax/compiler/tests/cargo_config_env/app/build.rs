// cargo hands [env] to build scripts too; a script's rustc-env wins over [env] in the crate.
fn main() {
    println!("cargo:rerun-if-env-changed=CCE_PLAIN");
    let plain = std::env::var("CCE_PLAIN").unwrap_or_else(|_| "unset".to_string());
    println!("cargo:rustc-env=CCE_SCRIPT=script saw {}", plain);
    println!("cargo:rustc-env=CCE_OVERRIDE=override from script");
}
