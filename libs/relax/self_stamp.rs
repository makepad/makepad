//! A Relax crate's own source stamp (#[path] from its build script; the build script declares
//! `sha256` at its root): writes `$OUT_DIR/src_stamp.rs` = `pub const SRC_STAMP: u128`, a hash of
//! every file of the package (nested packages, `target`, hidden entries and its tests, benches and
//! examples left out), the files `extra` names (inputs the build reads outside the package), and
//! the package's enabled features and how rustc compiles it (version, target, CPU features,
//! profile, flags). The crate `include!`s it, so the stamp is compiled into the
//! same rlib as the code it describes: a crate cargo did not rebuild keeps its old stamp with its
//! old code. relax-compiler folds every stamp of its closure into the codegen identity
//! (codegen_id.rs), the key of every stored build.
//! Reruns: every file it hashes, so the stamp changes exactly when the crate's inputs do.
use std::path::{Path, PathBuf};

fn files(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut rd: Vec<(String, bool)> = std::fs::read_dir(dir)
        .expect("read package directory")
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
        .collect();
    rd.sort();
    for (name, is_dir) in rd {
        if name.starts_with('.') {
            continue;
        }
        let p = dir.join(&name);
        let r = if rel.is_empty() { name.clone() } else { format!("{}/{}", rel, name) };
        if is_dir {
            if name == "target" || p.join("Cargo.toml").is_file() || (rel.is_empty() && matches!(name.as_str(), "tests" | "benches" | "examples")) {
                continue;
            }
            files(&p, &r, out);
        } else if p.is_file() {
            out.push((r, p));
        }
    }
}

/// Writes the stamp; `extra` = (label, bytes) of inputs outside the package.
pub fn write(extra: &[(String, Vec<u8>)]) {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mut list = Vec::new();
    files(&dir, "", &mut list);
    let mut buf = Vec::new();
    let put = |b: &[u8], buf: &mut Vec<u8>| {
        buf.extend_from_slice(&(b.len() as u64).to_le_bytes());
        buf.extend_from_slice(b);
    };
    put(b"relax-src-stamp 1", &mut buf);
    put(std::env::var("CARGO_PKG_NAME").unwrap().as_bytes(), &mut buf);
    for (rel, p) in &list {
        println!("cargo:rerun-if-changed={}", p.display());
        put(rel.as_bytes(), &mut buf);
        put(&std::fs::read(p).expect("read package file"), &mut buf);
    }
    // a directory watch sees files added to or removed from the package
    println!("cargo:rerun-if-changed={}", dir.join("src").display());
    for (label, bytes) in extra {
        put(label.as_bytes(), &mut buf);
        put(bytes, &mut buf);
    }
    let mut features: Vec<String> = std::env::vars().map(|(k, _)| k).filter(|k| k.starts_with("CARGO_FEATURE_")).collect();
    features.sort();
    for f in &features {
        put(f.as_bytes(), &mut buf);
    }
    // how rustc compiles it: toolchain, target, CPU features, profile, flags
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = std::process::Command::new(rustc).arg("-vV").output().expect("rustc -vV");
    assert!(version.status.success(), "rustc -vV failed");
    put(&version.stdout, &mut buf);
    for k in ["TARGET", "PROFILE", "OPT_LEVEL", "DEBUG", "CARGO_ENCODED_RUSTFLAGS", "CARGO_CFG_TARGET_FEATURE", "CARGO_CFG_DEBUG_ASSERTIONS", "CARGO_CFG_PANIC", "CARGO_CFG_OVERFLOW_CHECKS"] {
        put(k.as_bytes(), &mut buf);
        put(std::env::var(k).unwrap_or_default().as_bytes(), &mut buf);
    }
    let d = crate::sha256::sha256(&buf);
    let stamp = u128::from_le_bytes(d[..16].try_into().unwrap());
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("src_stamp.rs");
    let text = format!("/// This crate's source stamp (libs/relax/self_stamp.rs).\npub const SRC_STAMP: u128 = 0x{:032x};\n", stamp);
    if std::fs::read_to_string(&out).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&out, text).expect("write src_stamp.rs");
    }
}
