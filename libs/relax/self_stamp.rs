//! A Relax crate's own source stamp (#[path] from its build script; the build script declares
//! `sha256` at its root): writes `$OUT_DIR/src_stamp.rs` = `pub const SRC_STAMP: u128`, a hash of
//! the package's compiled files (Rust sources, Cargo.toml and literal include targets, as
//! srchash::compiled_files defines them), `extra` (additional build inputs), and
//! the package's enabled features and how rustc compiles it (version, target, CPU features,
//! profile, flags). The crate `include!`s it, so the stamp is compiled into the
//! same rlib as the code it describes: a crate cargo did not rebuild keeps its old stamp with its
//! old code. relax-compiler folds every stamp of its closure into the codegen identity
//! (codegen_id.rs), the key of every stored build.
//! Reruns: every file it hashes, so the stamp changes exactly when the crate's inputs do.
use std::path::PathBuf;

#[path = "base/src/srchash.rs"]
#[allow(dead_code)]
mod srchash;

// This module is mounted by build scripts that need no Relax VFS at their crate root.
mod vfs {
    pub fn read(p: impl AsRef<std::path::Path>) -> std::io::Result<Vec<u8>> {
        std::fs::read(p)
    }
    pub fn is_file(p: impl AsRef<std::path::Path>) -> bool {
        p.as_ref().is_file()
    }
    pub fn read_dir(p: impl AsRef<std::path::Path>) -> std::io::Result<Vec<(String, bool)>> {
        let mut v: Vec<(String, bool)> = std::fs::read_dir(p)?
            .flatten()
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.file_type().map(|t| t.is_dir()).unwrap_or(false)))
            .collect();
        v.sort();
        Ok(v)
    }
}

/// Writes the stamp; `extra` = (label, bytes) of inputs outside the package.
pub fn write(extra: &[(String, Vec<u8>)]) {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let list = srchash::compiled_files(&dir);
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
    // a source-directory watch sees additions and removals without watching the package root
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
