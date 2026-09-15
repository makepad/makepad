//! On-device validation + benchmark harness for the CUDA llama executor.
//! The harness (`canary.rs`) needs the CUDA store, a Linux/Windows-only
//! dependency; anywhere else this binary only says so.

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod canary;

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn main() {
    canary::main();
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn main() {
    eprintln!("llama-cuda-canary: the CUDA llama executor and its canary only exist on Linux/Windows");
    std::process::exit(2);
}
