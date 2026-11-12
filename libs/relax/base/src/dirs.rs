//! Where makepad run keeps what it writes: like cargo's build output, everything for a workspace
//! lives in `<target dir>/makepad-run` (the "run dir"), where the target dir is what `cargo metadata`
//! reports as `target_directory` (resolve::target_dir: CARGO_TARGET_DIR, then cargo config's
//! `build.target-dir`, then `<workspace root>/target`). `cargo clean` removes it with the rest of
//! the build output, and a fresh checkout is cold. There is no per-user cache.
//!
//! Inside the run dir:
//!   cache/                  the store files (code, front images, graph certificates,
//!                           the collected program image): store.rs
//!   registry/               crates.io packages downloaded for this workspace (registry.rs)
//!   git.lock                the commits git dependencies were pinned to when Cargo.lock does not
//!                           list them (resolve.rs; their stores are the run home's, gitstore.rs)
//!   startup/<pkg>.startup   the startup manifest of `makepad run --ahead=aot-max|aot-min`
//!   build-scripts/<hex>/, build/<crate>-<hex>/out   build script units and OUT_DIRs (as deep
//!                           as cargo's: scripts find the workspace 5 folders above OUT_DIR)
//!   serve/                  the `makepad serve` endpoint
//!
//! A git store run (the loader's `run <git url>`) has no workspace on disk: its workspace is
//! virtual (vfs.rs, `<run home>/vfs/<label>`). Its run dir is
//! `<run home>/target/<8 hex of the virtual workspace root>`, next to the loader's git stores
//! in the same per-user folder the loader installs to.
//!
//! MAKEPAD_RUN_CACHE_DIR replaces the whole run dir: for tests and measurement only. A run
//! that starts another run for the same program (a build script) hands its run dir over
//! with `makepad run --run-dir <dir>` (set_relax_dir).
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static SET: OnceLock<PathBuf> = OnceLock::new();
static SET_WORKSPACE: OnceLock<String> = OnceLock::new();
static DEFAULT: OnceLock<PathBuf> = OnceLock::new();
/// An absolute, lexically normalized anchor directory.
fn absolute_dir(dir: &str) -> String {
    let path = std::path::absolute(dir).unwrap_or_else(|_| PathBuf::from(dir));
    crate::vfs::normalize_path(&path.to_string_lossy())
}

/// The anchors, fixed once per process. std has none of its own: inside the workspace its files
/// are the workspace's, elsewhere (an installed runner's) its absolute path is one place for
/// every worktree that runs that install -- and a key must not change with when std is located.
fn anchors() -> &'static [(&'static str, String)] {
    static ANCHORS: OnceLock<Vec<(&'static str, String)>> = OnceLock::new();
    ANCHORS.get_or_init(|| crate::zone_alloc::kept_with(|| {
        let mut v = vec![("run", absolute_dir(&relax_dir().to_string_lossy())), ("cargo", absolute_dir(&crate::git::cargo::home().to_string_lossy()))];
        if let Ok(root) = crate::resolve::find_root(&workspace_dir()) { v.push(("ws", absolute_dir(&root))); }
        v
    }))
}

/// The longest normalized containing anchor, with component-wise containment on every OS.
fn anchor_from<'a>(path: &str, anchors: impl Iterator<Item = (&'a str, &'a str)>) -> Option<String> {
    anchors.filter_map(|(tag, dir)| {
        crate::vfs::relative_path(path, dir).map(|rel| (crate::vfs::normalize_path(dir).len(), tag, rel))
    }).max_by_key(|(len, _, _)| *len).map(|(_, tag, rel)| format!("\0{tag}\0{rel}"))
}

/// Chooses an anchored spelling from an explicit list, independent of process state.
fn anchor_in(path: &str, anchors: &[(&'static str, String)]) -> Option<String> {
    anchor_from(path, anchors.iter().map(|(tag, dir)| (*tag, dir.as_str())))
}

/// The persisted spelling of a path under its longest anchor; unrelated paths stay borrowed.
pub fn anchored(path: &str) -> std::borrow::Cow<'_, str> {
    anchor_in(path, anchors()).map_or(std::borrow::Cow::Borrowed(path), std::borrow::Cow::Owned)
}

/// The absolute anchor locations as one key input (the worktree).
pub fn anchors_key() -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str("relax-anchors");
    for (tag, dir) in anchors() { h.str(tag); h.str(dir); }
    h.finish128()
}

/// The run dir handed over by a parent run (`makepad run --run-dir`). Set before anything
/// asks for relax_dir().
pub fn set_relax_dir(dir: PathBuf) {
    // OUT_DIR and include! paths must be absolute, as Cargo's are.
    let dir = std::path::absolute(&dir).unwrap_or(dir);
    let _ = SET.set(crate::zone_alloc::kept_clone(&dir));
}

/// The workspace dir handed over by a parent run (`makepad run --workspace-dir`), leaving the
/// process's OS working directory unchanged.
pub fn set_workspace_dir(dir: String) {
    let _ = SET_WORKSPACE.set(crate::zone_alloc::kept_clone(&dir));
}

/// The workspace: `--workspace-dir` (`set_workspace_dir`), else MAKEPAD_RUN_WORKSPACE (a git run's
/// virtual folder, vfs.rs; it does not exist on disk, so it cannot be the current folder), else
/// the current folder.
pub fn workspace_dir() -> String {
    if let Some(w) = SET_WORKSPACE.get() {
        return w.clone();
    }
    if let Some(w) = std::env::var_os("MAKEPAD_RUN_WORKSPACE").filter(|w| !w.is_empty()) {
        return w.to_string_lossy().into_owned();
    }
    std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ".".to_string())
}

/// This process's relax dir: the handed-over one, else MAKEPAD_RUN_CACHE_DIR, else the workspace's
/// (relax_dir_for(workspace_dir())).
pub fn relax_dir() -> PathBuf {
    if let Some(d) = SET.get() {
        return d.clone();
    }
    if let Some(d) = override_dir() {
        return d;
    }
    DEFAULT.get_or_init(|| crate::zone_alloc::kept_with(|| relax_dir_for(&workspace_dir()))).clone()
}

/// A git store run's workspace before its stores are mounted (`root`: its outer mount to be, the
/// virtual `<run home>/vfs/<label>`): an app's start opens its window first, and the window keeps
/// its geometry and boot timings in the run dir, which is then the one the mounted run has.
pub fn preset_virtual_workspace(root: &str) {
    let _ = DEFAULT.set(crate::zone_alloc::kept_with(|| virtual_dir(&crate::vfs::normalize_path(root))));
}

/// The run dir of a virtual workspace whose outer mount is `root`.
fn virtual_dir(root: &str) -> PathBuf {
    // FNV-1a: short (Windows' 260-character path limit; build outputs nest below it)
    let mut h: u64 = 0xcbf29ce484222325;
    for b in root.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    crate::gitstore::relax_home().join("target").join(format!("{:08x}", h as u32))
}

/// MAKEPAD_RUN_CACHE_DIR when set (tests and measurement).
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("MAKEPAD_RUN_CACHE_DIR").filter(|d| !d.is_empty()).map(|dir| {
        let dir = PathBuf::from(dir);
        std::path::absolute(&dir).unwrap_or(dir)
    })
}

/// The relax dir of the workspace holding `start` (no override): `<cargo target dir>/makepad-run`; a
/// folder with no Cargo.toml at or above it: `<start>/target/makepad-run`; a virtual (git store)
/// workspace: `<relax home>/target/<8 hex>`.
pub fn relax_dir_for(start: &str) -> PathBuf {
    if crate::vfs::is_virtual(start) {
        return virtual_dir(&crate::vfs::outer_root(start).unwrap_or_else(|| crate::vfs::normalize_path(start)));
    }
    let target = crate::resolve::target_dir(start).unwrap_or_else(|_| format!("{}/target", crate::vfs::normalize_path(start)));
    Path::new(&target).join("makepad-run")
}

/// The store files' folder: `<relax dir>/cache`.
pub fn cache_dir() -> PathBuf {
    relax_dir().join("cache")
}

/// Where published crates' sources live: the cargo home's `registry/` and the relax dir's
/// (registry.rs).
pub fn registry_dirs() -> [PathBuf; 2] {
    [crate::git::cargo::home().join("registry"), relax_dir().join("registry")]
}

#[cfg(test)]
mod tests {
    /// Anchors use normalized components, prefer nested roots and recognize Windows drives.
    #[test]
    fn paths_choose_the_longest_anchor() {
        let anchors = [("ws", "/a/./ws/".to_string()), ("run", "/a/ws/target/run".to_string())];
        assert_eq!(super::anchor_in("/a/ws/f.rs", &anchors).as_deref(), Some("\0ws\0f.rs"));
        assert_eq!(super::anchor_in("/a/ws/target/run/f.rs", &anchors).as_deref(), Some("\0run\0f.rs"));
        assert_eq!(super::anchor_in("/a/ws/std/core/lib.rs", &anchors).as_deref(), Some("\0ws\0std/core/lib.rs"));
        assert_eq!(super::anchor_in("/b/f.rs", &anchors), None);
        assert_eq!(super::anchor_in("/a/wsx/f.rs", &anchors), None);
        assert_eq!(super::anchor_in("C:/a/ws/f.rs", &[("ws", "C:/a/ws".to_string())]).as_deref(), Some("\0ws\0f.rs"));
    }
}
