//! Which baked library the wall opens.
//!
//! A library is a directory the image-tiles baker made (`library.sqlite`,
//! `tapes/`, `full/`, `pyr/`). The app looks, in order, at what
//! `IMAGE_TILES_HOME` names, the nearest `local/image-tiles` walking up
//! from the working directory (the way every app launched from a source
//! tree finds `local/`), and the same directory under the checkout the
//! running binary was built in — so a copy of the binary outside the tree
//! still finds the tree's pictures. Under that root the named collection
//! wins (`smbc`, the comic archive), else the root itself when it is a
//! library. Nothing found means an empty wall with a status line saying
//! how to bake one; the app never bakes on its own.

use makepad_image_tiles::Library;
use std::path::{Path, PathBuf};

/// The collection the app opens by default: the SMBC comic archive.
pub const DEFAULT_COLLECTION: &str = "smbc";

/// The library to open for `collection` (`None` = the default one), or
/// `None` when no baked library exists in any of the places we look.
pub fn find(collection: Option<&str>) -> Option<Library> {
    let name = collection.unwrap_or(DEFAULT_COLLECTION);
    for base in candidate_roots() {
        if let Some(library) = library_under(&base, name) {
            return Some(library);
        }
    }
    None
}

/// The library for `name` under `base`: `<base>/<name>` when that is a
/// baked library, else `base` itself when it is one.
pub fn library_under(base: &Path, name: &str) -> Option<Library> {
    if is_collection_name(name) {
        let named = Library::new(base.join(name));
        if named.exists() {
            return Some(named);
        }
    }
    let root = Library::new(base);
    root.exists().then_some(root)
}

/// A collection is a plain name, never a path: the assistant may ask for
/// one by name and the host resolves it under the library root.
pub fn is_collection_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The roots to look under, most specific first.
pub fn candidate_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // `IMAGE_TILES_HOME` and the working directory's `local/`, as the
    // image-tiles crate resolves them.
    out.push(Library::resolve().root);
    // The checkout the binary was built in, walking up from the executable
    // (target/release/photos → the repo root with `Cargo.toml` + `local/`).
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..4 {
            let Some(d) = dir else { break };
            if d.join("Cargo.toml").exists() && d.join("local").is_dir() {
                let root = d.join("local").join("image-tiles");
                if !out.contains(&root) {
                    out.push(root);
                }
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    out
}

/// The one line the status shows when nothing is baked.
pub fn how_to_bake() -> String {
    format!(
        "No picture library found. Bake one: image-tiles-bake --root local/image-tiles/{DEFAULT_COLLECTION} <manifest.tsv>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_names_are_plain() {
        assert!(is_collection_name("smbc"));
        assert!(is_collection_name("holiday_2026-08"));
        assert!(!is_collection_name(""));
        assert!(!is_collection_name("../etc"));
        assert!(!is_collection_name("/abs/path"));
        assert!(!is_collection_name("a b"));
    }

    #[test]
    fn a_named_collection_under_a_root_wins_over_the_root_and_nothing_is_a_miss() {
        let dir = std::env::temp_dir().join(format!("photos-lib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("smbc")).unwrap();
        // Nothing baked anywhere yet.
        assert!(library_under(&dir, "smbc").is_none());
        // The root itself is a library: found when the name has nothing.
        std::fs::write(dir.join("library.sqlite"), b"").unwrap();
        assert_eq!(library_under(&dir, "smbc").unwrap().root, dir);
        // The named collection is baked: it wins.
        std::fs::write(dir.join("smbc").join("library.sqlite"), b"").unwrap();
        assert_eq!(library_under(&dir, "smbc").unwrap().root, dir.join("smbc"));
        // A path is never a collection name: it falls back to the root.
        assert_eq!(library_under(&dir, "../x").unwrap().root, dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!candidate_roots().is_empty());
    }
}
