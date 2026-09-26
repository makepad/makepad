//! Which baked library the wall opens.
//!
//! A library is a directory the image-tiles baker made (`library.sqlite`,
//! `tapes/`, `full/`, `pyr/`). The app looks, in order, at what
//! `IMAGE_TILES_HOME` names, the nearest `local/image-tiles` walking up
//! from the working directory (the way every app launched from a source
//! tree finds `local/`), the same directory under the checkout the running
//! binary was built in (a copy of the binary outside the tree still finds
//! the tree's pictures), the makepad home's `photos/image-tiles` (where an
//! installed app, and a phone, keep their own), and last a set a build
//! bundles with the crate (`resources/image-tiles`; the repository ships
//! none). Under a root the named collection wins, else the root itself when
//! it is a library, else the first collection baked under it. Nothing found
//! means an empty wall that says where a library would go; the app never
//! bakes on its own.

use makepad_image_tiles::Library;
use std::path::{Path, PathBuf};

/// The collection the app opens by default (any other baked collection
/// under the same root opens when there is none by this name).
pub const DEFAULT_COLLECTION: &str = "photos";

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
/// baked library, else `base` itself when it is one, else the first
/// collection (by name) baked directly under `base`.
pub fn library_under(base: &Path, name: &str) -> Option<Library> {
    if is_collection_name(name) {
        let named = Library::new(base.join(name));
        if named.exists() {
            return Some(named);
        }
    }
    let root = Library::new(base);
    if root.exists() {
        return Some(root);
    }
    let mut names: Vec<String> = std::fs::read_dir(base).ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| is_collection_name(name))
        .collect();
    names.sort();
    names.into_iter().map(|name| Library::new(base.join(name))).find(Library::exists)
}

/// A collection is a plain name, never a path: the assistant may ask for
/// one by name and the host resolves it under the library root.
pub fn is_collection_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The roots to look under, most specific first: the checkout's, then the
/// makepad home's, then the bundle's (see [`roots_in_order`]).
pub fn candidate_roots() -> Vec<PathBuf> {
    let mut checkout = Vec::new();
    // `IMAGE_TILES_HOME` and the working directory's `local/`, as the
    // image-tiles crate resolves them.
    checkout.push(Library::resolve().root);
    // The checkout the binary was BUILT in, by its compiled-in manifest
    // path: a simulator app runs on the very machine that built it and can
    // read that checkout's full set (the symlinked pictures the bundle
    // leaves out), while on a device or another machine the path is simply
    // absent. Then the checkout found by walking up from the executable
    // (target/release/photos → the repo root with `Cargo.toml` + `local/`).
    let built_in = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo) = built_in.parent().and_then(Path::parent) {
        if repo.join("Cargo.toml").exists() && repo.join("local").is_dir() {
            let root = repo.join("local").join("image-tiles");
            if !checkout.contains(&root) {
                checkout.push(root);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(Path::to_path_buf);
        for _ in 0..4 {
            let Some(d) = dir else { break };
            if d.join("Cargo.toml").exists() && d.join("local").is_dir() {
                let root = d.join("local").join("image-tiles");
                if !checkout.contains(&root) {
                    checkout.push(root);
                }
                break;
            }
            dir = d.parent().map(Path::to_path_buf);
        }
    }
    roots_in_order(checkout, home_root(), bundled_root())
}

/// The order every host resolves in: the checkout's roots, the home's,
/// the bundle's. Pure, so the order is testable without a filesystem.
pub fn roots_in_order(checkout: Vec<PathBuf>, home: PathBuf, bundled: Option<PathBuf>) -> Vec<PathBuf> {
    let mut out = checkout;
    if !out.contains(&home) {
        out.push(home);
    }
    if let Some(bundled) = bundled {
        if !out.contains(&bundled) {
            out.push(bundled);
        }
    }
    out
}

/// Where an installed app — and a phone — keeps its own libraries: the
/// makepad home's `photos/image-tiles`, one collection per directory.
pub fn home_root() -> PathBuf {
    makepad_widgets::makepad_platform::home::makepad_home().join("photos").join("image-tiles")
}

/// The set bundled with this crate, when the build carries it: the crate's
/// `resources/image-tiles`. Out of a checkout that is the source tree's copy
/// (the manifest dir is baked into the binary); packaged, it is what
/// cargo-makepad laid out beside the executable under
/// `makepad/<crate>/resources/` (an iOS .app is that flat; a macOS .app
/// keeps it under `Contents/Resources`). `None` when no build carries it.
pub fn bundled_root() -> Option<PathBuf> {
    // The packaged copy beside the executable comes FIRST: the checkout
    // path compiled in below exists on the machine that built a simulator
    // app too, and lies outside that app's sandbox — a directory that is
    // there but cannot be opened. Only a binary run out of its checkout
    // (no package around it) falls back to the manifest's resources.
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let packaged = Path::new("makepad").join("makepad_photos").join("resources").join("image-tiles");
            candidates.push(dir.join(&packaged));
            candidates.push(dir.join("..").join("Resources").join(&packaged));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources").join("image-tiles"));
    candidates.into_iter().find(|root| root.is_dir())
}

/// Where a library would go, said for a person on a phone: the home's
/// `photos/image-tiles/<collection>`, as a path tail — never a command line.
pub fn where_a_library_goes(collection: Option<&str>) -> String {
    let name = collection.filter(|c| is_collection_name(c)).unwrap_or(DEFAULT_COLLECTION);
    format!("Put a baked library in photos/image-tiles/{name} under the makepad home.")
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
        assert!(is_collection_name("photos"));
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
        std::fs::create_dir_all(dir.join("photos")).unwrap();
        // Nothing baked anywhere yet.
        assert!(library_under(&dir, "photos").is_none());
        // Another collection is baked: it opens when the named one is not.
        std::fs::create_dir_all(dir.join("holiday")).unwrap();
        std::fs::write(dir.join("holiday").join("library.sqlite"), b"").unwrap();
        assert_eq!(library_under(&dir, "photos").unwrap().root, dir.join("holiday"));
        // The root itself is a library: found when the name has nothing.
        std::fs::write(dir.join("library.sqlite"), b"").unwrap();
        assert_eq!(library_under(&dir, "photos").unwrap().root, dir);
        // The named collection is baked: it wins.
        std::fs::write(dir.join("photos").join("library.sqlite"), b"").unwrap();
        assert_eq!(library_under(&dir, "photos").unwrap().root, dir.join("photos"));
        // A path is never a collection name: it falls back to the root.
        assert_eq!(library_under(&dir, "../x").unwrap().root, dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!candidate_roots().is_empty());
    }

    #[test]
    fn the_checkout_wins_over_the_home_and_the_home_over_the_bundle() {
        let checkout = PathBuf::from("/checkout/local/image-tiles");
        let home = PathBuf::from("/home/.makepad/photos/image-tiles");
        let bundled = PathBuf::from("/app/makepad/makepad_photos/resources/image-tiles");
        assert_eq!(
            roots_in_order(vec![checkout.clone()], home.clone(), Some(bundled.clone())),
            vec![checkout.clone(), home.clone(), bundled.clone()]
        );
        // No checkout root at all (a phone): the home, then the bundle.
        assert_eq!(roots_in_order(Vec::new(), home.clone(), Some(bundled.clone())), vec![home.clone(), bundled.clone()]);
        // A build without the bundle stops at the home; duplicates fold.
        assert_eq!(roots_in_order(vec![home.clone()], home.clone(), None), vec![home]);
        // The first root that HAS the collection wins, in that order.
        let dir = std::env::temp_dir().join(format!("photos-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(b.join("photos")).unwrap();
        std::fs::write(b.join("photos").join("library.sqlite"), b"").unwrap();
        let found = roots_in_order(vec![a.clone()], b.clone(), None).iter().find_map(|r| library_under(r, "photos")).unwrap();
        assert_eq!(found.root, b.join("photos"));
        std::fs::create_dir_all(a.join("photos")).unwrap();
        std::fs::write(a.join("photos").join("library.sqlite"), b"").unwrap();
        let found = roots_in_order(vec![a.clone()], b.clone(), None).iter().find_map(|r| library_under(r, "photos")).unwrap();
        assert_eq!(found.root, a.join("photos"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(where_a_library_goes(None).contains("photos/image-tiles/photos"));
        assert!(where_a_library_goes(Some("../x")).contains("photos/image-tiles/photos"));
        assert!(!where_a_library_goes(None).contains("image-tiles-bake"));
    }
}
