//! relax-compiler's build: MAKEPAD_RUN_PROVENANCE (build_stamp.rs: content hashes of the packages
//! this crate compiles and of the std it ships with, never git state) and MAKEPAD_RUN_STD_DIGEST.
use std::path::Path;

#[path = "../base/src/json.rs"]
#[allow(dead_code)]
mod json;
#[path = "../base/src/srchash.rs"]
#[allow(dead_code)]
mod srchash;
#[path = "../build_stamp.rs"]
mod build_stamp;
use build_stamp::vfs;
#[path = "../base/src/sha256.rs"]
mod sha256;
#[path = "../base/src/std_digest.rs"]
mod std_digest;

fn std_digest(dir: &Path) -> String {
    fn collect(root: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("read std directory") {
            let entry = entry.expect("std entry");
            let path = entry.path();
            let kind = entry.file_type().expect("std file type");
            if kind.is_dir() {
                collect(root, &path, files);
            } else {
                assert!(kind.is_file(), "std must contain only regular files: {}", path.display());
                let rel = path.strip_prefix(root).unwrap().to_str().expect("UTF-8 std path").replace('\\', "/");
                files.push((rel, std::fs::read(&path).expect("read std file")));
            }
        }
    }
    let root = dir.join("std");
    // Watching the directory includes file additions and removals, not just modifications.
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    collect(&root, &root, &mut files);
    std_digest::digest(files)
}

fn main() {
    // (the Windows main-thread stack is the `makepad` binary's: tools/run/build.rs; a library
    // has no bin to give link args to)
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../build_stamp.rs");
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let std = std_digest(&dir);
    println!("cargo:rustc-env=MAKEPAD_RUN_STD_DIGEST={}", std);
    let g = build_stamp::graph(&dir);
    println!("cargo:rustc-env=MAKEPAD_RUN_PROVENANCE={}", g.provenance(&[dir.parent().unwrap(), &dir.join("../../../tools/run")], &format!("std {}", std)));
}
