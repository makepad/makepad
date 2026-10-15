// Two small pure-Rust crates.io packages, not vendored, not in CARGO_HOME (tests/registry_fetch/run.sh).
use std::hash::Hasher;

cfg_if::cfg_if! {
    if #[cfg(target_pointer_width = "64")] {
        const BITS: u32 = 64;
    } else {
        const BITS: u32 = 32;
    }
}

fn main() {
    let mut h = fnv::FnvHasher::default();
    h.write(b"relax");
    println!("registry ok {:016x} {}", h.finish(), BITS);
}
