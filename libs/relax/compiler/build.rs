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
#[path = "../self_stamp.rs"]
mod self_stamp;

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
    println!("cargo:rerun-if-changed=../base/src/json.rs");
    println!("cargo:rerun-if-changed=../base/src/srchash.rs");
    println!("cargo:rerun-if-changed=../base/src/sha256.rs");
    println!("cargo:rerun-if-changed=../build_stamp.rs");
    let dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let std = std_digest(&dir);
    println!("cargo:rustc-env=MAKEPAD_RUN_STD_DIGEST={}", std);
    let g = build_stamp::graph(&dir);
    println!("cargo:rustc-env=MAKEPAD_RUN_PROVENANCE={}", g.provenance(&[dir.parent().unwrap(), &dir.join("../../../tools/run")], &format!("std {}", std)));
    println!("cargo:rerun-if-changed=../self_stamp.rs");
    self_stamp::write(&[]);
    codegen_parts(&g, &dir);
}

/// `$OUT_DIR/codegen_parts.rs` (src/codegen_id.rs): what decides the code Relax generates, i.e. this
/// crate's library closure. A Relax crate (libs/relax) is named by the source stamp compiled into
/// its own rlib (self_stamp.rs); every other package of the closure (makepad-remote, the proc-macro
/// impls the native macros run, ...) is hashed here: its files, version and resolved features. A
/// Relax crate added to the closure without a stamp, or not a direct dependency of this crate, is
/// a compile error, so the identity cannot silently leave one out.
fn codegen_parts(g: &build_stamp::Graph, dir: &Path) {
    let relax = dir.parent().unwrap().canonicalize().unwrap();
    let mut stamped = Vec::new();
    let mut others = Vec::new();
    for i in g.closure(g.root) {
        let p = &g.pkgs[i];
        let pdir = Path::new(&p.dir).canonicalize().unwrap_or_else(|_| p.dir.clone().into());
        if pdir.starts_with(&relax) {
            stamped.push(p.name.clone());
            continue;
        }
        let mut buf = Vec::new();
        let mut put = |b: &[u8]| {
            buf.extend_from_slice(&(b.len() as u64).to_le_bytes());
            buf.extend_from_slice(b);
        };
        put(p.name.as_bytes());
        put(p.version.as_bytes());
        let mut features = p.features.clone();
        features.sort();
        put(features.join(",").as_bytes());
        if !p.dir.contains("/.cargo/") {
            let mut files = Vec::new();
            srchash::files(Path::new(&p.dir), "", &mut files);
            files.retain(|(rel, _)| !["tests/", "benches/", "examples/"].iter().any(|d| rel.starts_with(d)));
            files.sort();
            for (rel, f) in &files {
                println!("cargo:rerun-if-changed={}", f.display());
                put(rel.as_bytes());
                put(&std::fs::read(f).expect("read package file"));
            }
        }
        let d = sha256::sha256(&buf);
        others.push((p.name.clone(), u128::from_le_bytes(d[..16].try_into().unwrap())));
    }
    stamped.sort();
    others.sort();
    let mut out = String::from("/// What decides the code this build generates (build.rs codegen_parts): (package, stamp).\npub(crate) const CODEGEN_PARTS: &[(&str, u128)] = &[\n");
    for name in &stamped {
        let lib = if name == "relax-compiler" { "crate".to_string() } else { name.replace('-', "_") };
        out.push_str(&format!("    ({:?}, {}::SRC_STAMP),\n", name, lib));
    }
    for (name, h) in &others {
        out.push_str(&format!("    ({:?}, 0x{:032x}),\n", name, h));
    }
    out.push_str("];\n");
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("codegen_parts.rs");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(out.as_str()) {
        std::fs::write(&path, out).expect("write codegen_parts.rs");
    }
}
