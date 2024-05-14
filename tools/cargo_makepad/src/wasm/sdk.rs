use crate::makepad_shell::*;

pub fn rustup_toolchain_install() -> Result<(), String> {
    println!("Installing Rust toolchains for wasm");
    /*
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "update",
    ]) ?;*/
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "install",
        "nightly"
    ]) ?;
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "target",
        "add",
        "wasm32-unknown-unknown",
        "--toolchain",
        "nightly"
    ]) ?;
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "target",
        "add",
        "wasm32-unknown-unknown",
        "--toolchain",
        "nightly"
    ]) ?;
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "component",
        "add",
        "rust-std",
        "--toolchain",
        "nightly",
        "--target",
        "wasm32-unknown-unknown"
    ]) ?;
    shell_env(&[],&std::env::current_dir().unwrap(), "rustup", &[
        "component",
        "add",
        "rust-src",
        "--toolchain",
        "nightly",
        "--target",
        "wasm32-unknown-unknown"
    ]) ?;
    
    Ok(())
}

//MAKEPAD=lines RUSTFLAGS="-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z" cargo +nightly build $1 $2 --target=wasm32-unknown-unknown --release -Z build-std=panic_abort,std

