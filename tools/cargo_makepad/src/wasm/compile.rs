use crate::makepad_shell::*;

pub fn build(args: &[String]) -> Result<(), String> {
    
    let base_args = &[
        "run",
        "nightly",
        "cargo",
        "build",
        "--target=wasm32-unknown-unknown",
        "-Z",
        "build-std=panic_abort,std"
    ];
    
    let cwd = std::env::current_dir().unwrap();
    
    let mut args_out = Vec::new();
    args_out.extend_from_slice(base_args);
    for arg in args {
        args_out.push(arg);
    }
    
    shell_env(&[
        ("RUSTFLAGS", "-C codegen-units=1 -C target-feature=+atomics,+bulk-memory,+mutable-globals -C link-arg=--export=__stack_pointer -C opt-level=z"),
        ("MAKEPAD", "lines"),
    ], &cwd, "rustup", &args_out) ?;
    
    println!("WebAssembly build completed");
    
    Ok(())
}

pub fn run(_args: &[String]) -> Result<(), String> {
    return Err("Run is not implemented yet".into());
}
