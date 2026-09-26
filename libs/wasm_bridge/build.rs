use std::env;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;

fn main() {
    // The file below depends only on where the package and the target
    // directory are. Without a rerun-if line Cargo reruns this script
    // whenever any file in the package changes (a README, a test), and each
    // rerun rebuilds the crate.
    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = env::var("OUT_DIR").unwrap();
    let path = Path::new(&out_dir)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let cwd = std::env::current_dir().unwrap();
    let mut file = File::create(path.join("makepad-wasm-bridge.path")).unwrap();
    file.write_all(&format!("{}", cwd.display()).as_bytes())
        .unwrap();
}
