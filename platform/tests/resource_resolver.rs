//! The path-escape suite for `ResourceResolver` (KERNELS.md §5.2, P0-S):
//! every way a document's resource name could reach outside its root is
//! refused, and decoder budgets refuse overflowing or oversized images
//! before anything is allocated.
use makepad_platform::resource_resolver::*;
use std::path::PathBuf;

fn fresh_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("resource_resolver").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// root/doc/{a.txt, sub/b.txt} and root/secret.txt beside the document root.
fn fixture(name: &str) -> (PathBuf, ResourceResolver) {
    let base = fresh_dir(name);
    let doc = base.join("doc");
    std::fs::create_dir_all(doc.join("sub")).unwrap();
    std::fs::write(doc.join("a.txt"), b"alpha").unwrap();
    std::fs::write(doc.join("sub/b.txt"), b"beta").unwrap();
    std::fs::write(base.join("secret.txt"), b"secret").unwrap();
    let resolver = ResourceResolver::new(&doc).unwrap();
    (base, resolver)
}

#[test]
fn plain_relative_names_resolve_beneath_the_root() {
    let (_base, r) = fixture("plain");
    assert_eq!(r.read("a.txt", 100).unwrap(), b"alpha");
    assert_eq!(r.read("sub/b.txt", 100).unwrap(), b"beta");
    assert_eq!(r.read("./sub/./b.txt", 100).unwrap(), b"beta");
    assert_eq!(r.read("sub//b.txt", 100).unwrap(), b"beta");
    assert!(r.resolve("sub/b.txt").unwrap().starts_with(r.root()));
}

#[test]
fn escapes_are_refused() {
    let (base, r) = fixture("escapes");
    let secret = base.join("secret.txt");
    for name in [
        "../secret.txt",
        "sub/../../secret.txt",
        "sub/../a.txt", // `..` is refused even when it would stay inside
        "..",
        secret.to_str().unwrap(),
        "/etc/passwd",
        "//etc/passwd",
        "sub\\..\\..\\secret.txt",
        "C:secret.txt",
        "C:/Windows/win.ini",
        "a.txt:stream",
        "a.txt\0.png",
        "",
        ".",
        "./",
    ] {
        let err = r.resolve(name).expect_err(name);
        assert!(matches!(err, ResourceError::InvalidPath(_)), "{name:?}: {err}");
    }
    let long = "a/".repeat(600) + "x";
    assert!(matches!(r.resolve(&long), Err(ResourceError::InvalidPath(_))));
}

#[cfg(unix)]
#[test]
fn symlinks_are_never_followed() {
    let (base, r) = fixture("symlinks");
    let doc = r.root().to_path_buf();
    // A link to a file outside the root.
    std::os::unix::fs::symlink(base.join("secret.txt"), doc.join("out.txt")).unwrap();
    // A link to a directory outside the root, then a name through it.
    std::os::unix::fs::symlink(&base, doc.join("up")).unwrap();
    // A link that stays inside the root is refused too: links are not
    // followed at all, so no check depends on where they point.
    std::os::unix::fs::symlink(doc.join("a.txt"), doc.join("alias.txt")).unwrap();
    std::os::unix::fs::symlink(doc.join("sub"), doc.join("subalias")).unwrap();
    for name in ["out.txt", "up/secret.txt", "up", "alias.txt", "subalias/b.txt"] {
        let err = r.read(name, 100).expect_err(name);
        assert!(matches!(err, ResourceError::Symlink(_)), "{name}: {err}");
    }
}

#[test]
fn missing_files_directories_and_oversize_reads_fail_cleanly() {
    let (_base, r) = fixture("reads");
    assert!(matches!(r.read("nope.txt", 100), Err(ResourceError::Io(_))));
    assert!(matches!(r.read("sub", 100), Err(ResourceError::InvalidPath(_))));
    assert!(matches!(r.read("a.txt", 4), Err(ResourceError::Budget(_))));
    assert_eq!(r.read("a.txt", 5).unwrap(), b"alpha");
}

#[test]
fn asset_ids_follow_the_grammar() {
    for ok in ["abc", "ABC-123_x", "tex.v2", "a", &"x".repeat(128)] {
        assert!(validate_asset_id(ok).is_ok(), "{ok}");
    }
    for bad in [
        "", "../x", "..", ".hidden", "a/b", "a\\b", "a..b", "a b", "é", "a:b", "a\0b", "/abs", &"x".repeat(129),
    ] {
        assert!(matches!(validate_asset_id(bad), Err(ResourceError::InvalidAssetId(_))), "{bad:?}");
    }
}

#[test]
fn decode_budgets_refuse_before_allocating() {
    let mut budget = DecodeBudget::new(1000);
    assert_eq!(budget.charge_image(10, 10, 4, "img").unwrap(), 400);
    assert_eq!(budget.remaining(), 600);
    // Over the remaining budget: refused, nothing taken.
    assert!(matches!(budget.charge_image(20, 10, 4, "img"), Err(ResourceError::Budget(_))));
    assert_eq!(budget.used(), 400);
    // Overflowing sizes are errors, never wrapped into small numbers.
    assert!(DecodeBudget::image_bytes(1 << 33, 1 << 31, 4).is_err());
    assert!(DecodeBudget::image_bytes(u64::MAX, 2, 1).is_err());
    assert!(budget.charge_image(u64::MAX, u64::MAX, 16, "bomb").is_err());
    budget.release(400);
    assert_eq!(budget.used(), 0);
    assert!(budget.charge(1000, "all").is_ok());
    assert!(budget.charge(1, "one more").is_err());
}

#[test]
fn absolute_paths_are_read_only_inside_trusted_roots() {
    let (base, r) = fixture("trusted");
    let roots = [r.clone()];
    // Inside the root, spelled with `..` through the root itself: fine.
    let spelled = base.join("doc/sub/../a.txt");
    assert_eq!(ResourceResolver::read_within(&roots, &spelled, 100).unwrap(), b"alpha");
    assert_eq!(ResourceResolver::read_within(&roots, &base.join("doc/sub/b.txt"), 100).unwrap(), b"beta");
    // Outside every root, or relative: refused.
    let err = ResourceResolver::read_within(&roots, &base.join("secret.txt"), 100).unwrap_err();
    assert!(matches!(err, ResourceError::Escape(_)), "{err}");
    assert!(ResourceResolver::read_within(&roots, &base.join("doc/../secret.txt"), 100).is_err());
    assert!(ResourceResolver::read_within(&roots, std::path::Path::new("a.txt"), 100).is_err());
    assert!(ResourceResolver::read_within(&[], &base.join("doc/a.txt"), 100).is_err());
    #[cfg(unix)]
    {
        // A link inside the root pointing out resolves outside: refused.
        std::os::unix::fs::symlink(base.join("secret.txt"), r.root().join("out.txt")).unwrap();
        assert!(ResourceResolver::read_within(&roots, &r.root().join("out.txt"), 100).is_err());
    }
}
