//! A `Path` hashes as real std's does: the byte runs between separators, a `.` component after a
//! separator skipped, then a mix of the run lengths. The expected values are what rustc's std
//! (nightly 2026-10-26, `DefaultHasher::new()`) prints for each path, so a hash a Relax program
//! computes matches one from a rustc-built program, and paths that compare equal hash equal.
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

fn hash(p: &str) -> u64 {
    let mut h = DefaultHasher::new();
    Path::new(p).hash(&mut h);
    h.finish()
}

const RUSTC: [(&str, u64); 14] = [
    ("assets/ground/manifest.toml", 15732277487035833968),
    ("/a//b/./c", 15731394572416388128),
    ("/a/b/c", 15731394572416388128),
    ("a/b/", 16570388023511652081),
    ("a/b", 16570388023511652081),
    ("./x", 14204672513820937514),
    ("x", 3192109947553801569),
    ("foo/bar", 14281475280031871083),
    ("foobar", 11206374673574509708),
    ("", 13646096770106105413),
    ("/", 13646096770106105413),
    (".", 15596436556359631077),
    ("a/./b/.", 16570388023511652081),
    ("../a", 13317292434421979744),
];

#[cfg(unix)]
#[test]
fn path_hash_matches_rustc() {
    for (p, expected) in RUSTC {
        assert_eq!(hash(p), expected, "DefaultHasher of Path::new({p:?})");
    }
}

#[test]
fn equal_paths_hash_equal() {
    for (a, b) in [("/a//b/./c", "/a/b/c"), ("a/b/", "a/b"), ("a/./b/.", "a/b")] {
        assert_eq!(Path::new(a), Path::new(b), "{a:?} == {b:?}");
        assert_eq!(hash(a), hash(b), "hash({a:?}) == hash({b:?})");
    }
    assert_ne!(hash("foo/bar"), hash("foobar"), "a split path is not its joined bytes");
}
