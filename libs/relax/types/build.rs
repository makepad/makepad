//! This crate's source stamp (../self_stamp.rs): part of the codegen identity that keys Relax's stored builds.
#[path = "../base/src/sha256.rs"]
#[allow(dead_code)]
mod sha256;
#[path = "../self_stamp.rs"]
mod self_stamp;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../self_stamp.rs");
    // an input outside the package (src includes it)
    let extra = "../compiler/std/stability.tsv";
    println!("cargo:rerun-if-changed={}", extra);
    self_stamp::write(&[(extra.to_string(), std::fs::read(extra).expect("read included file"))]);
}
