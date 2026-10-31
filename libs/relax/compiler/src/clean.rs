//! `makepad clean [-p <package>]`: removes the workspace's run dir, `<target>/makepad-run` (dirs.rs),
//! and nothing else: never cargo's own build output next to it.
//!
//! The workspace is found as for the cache (cargo's rules, from the current folder). Refused:
//! under MAKEPAD_RUN_CACHE_DIR (an override for tests and measurement; clean never deletes an arbitrary
//! folder), and unless the resolved folder (symlinks resolved, the `makepad-run` folder itself not a
//! link) is `<root>/target/makepad-run` where `<root>` holds a Cargo.toml that has `[workspace]` or is
//! the package root cargo resolves -- or, with a target dir set by CARGO_TARGET_DIR /
//! CARGO_BUILD_TARGET_DIR / cargo config, exactly `<that target dir>/makepad-run`.
//!
//! The tree is removed by walking it (no shell, no glob; links are removed, never followed).
//! This includes every entry left by an older layout, not just the current store files:
//! `startup/`, `serve/` and any other directory inside `<target>/makepad-run` all go.
//! A file that cannot be removed (on Windows: mapped or open by a running app or makepad run) is
//! skipped and reported: the next start's prune removes stale store generations, and a later
//! clean the rest. Exit 0 with a summary.
//!
//! `-p <package>`: only that app's files: its startup manifest (`startup/<package>.startup`)
//! and its stores (cache.rs app_store_files: the app crate's own stores and its program's graph
//! stores). The stores of the crates the workspace's apps share, the registry packages and the
//! build script outputs stay.
use std::path::{Path, PathBuf};

pub fn cmd_clean(args: &[String]) -> i32 {
    let mut package = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" | "--package" if i + 1 < args.len() => {
                package = Some(args[i + 1].clone());
                i += 1;
            }
            _ => {
                eprintln!("usage: makepad clean [-p <package>]");
                return 2;
            }
        }
        i += 1;
    }
    let dir = match checked_dir(&crate::dirs::workspace_dir(), crate::dirs::override_dir().is_some()) {
        Ok(Some(d)) => d,
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("makepad clean: {e}");
            return 1;
        }
    };
    let mut r = Removal::default();
    match &package {
        None => r.tree(&dir),
        Some(p) => {
            r.file(&dir.join("startup").join(format!("{p}.startup")));
            for f in crate::cache::app_store_files(&dir.join("cache"), &p.replace('-', "_")) {
                r.file(&f);
            }
        }
    }
    r.report(&dir, package.as_deref());
    0
}

/// The relax dir to clean, checked (None: there is none, said so). `overridden`: MAKEPAD_RUN_CACHE_DIR
/// is set.
pub fn checked_dir(workspace: &str, overridden: bool) -> Result<Option<PathBuf>, String> {
    if overridden {
        return Err("MAKEPAD_RUN_CACHE_DIR is set: that override is for tests and measurement only, and clean never deletes an arbitrary folder (unset it, or remove that folder yourself)".to_string());
    }
    if crate::vfs::is_virtual(workspace) {
        return Err(format!("{workspace} is a git repository run's virtual workspace: no target folder to clean"));
    }
    let target = crate::resolve::target_dir(workspace)?;
    let default = format!("{}/target", crate::resolve::find_root(workspace)?);
    let custom = crate::vfs::normalize_path(&target) != crate::vfs::normalize_path(&default);
    let dir = Path::new(&target).join("makepad-run");
    let meta = match std::fs::symlink_metadata(&dir) {
        Ok(m) => m,
        Err(_) => {
            println!("makepad clean: {} does not exist: nothing to remove", dir.display());
            return Ok(None);
        }
    };
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link or a file): refused", dir.display()));
    }
    let real = std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let parent = std::fs::canonicalize(&target).map_err(|e| format!("{target}: {e}"))?;
    if real != parent.join("makepad-run") {
        return Err(format!("{} resolves to {}, outside {}: refused", dir.display(), real.display(), parent.display()));
    }
    if !same_name(real.file_name(), "makepad-run") {
        return Err(format!("{} is not named `makepad-run`: refused", real.display()));
    }
    if custom {
        println!("makepad clean: the target dir is set (CARGO_TARGET_DIR, CARGO_BUILD_TARGET_DIR or cargo config build.target-dir): cleaning {}", real.display());
        return Ok(Some(real));
    }
    if !same_name(parent.file_name(), "target") {
        return Err(format!("{} is not inside a folder named `target`: refused", real.display()));
    }
    let root = parent.parent().ok_or_else(|| format!("{}: no workspace folder above it", real.display()))?;
    let manifest = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).map_err(|_| format!("{} has no Cargo.toml: refused", root.display()))?;
    let is_workspace = crate::toml::parse_toml(&text).map_or(false, |d| d.root.contains_key("workspace"));
    let resolved = std::fs::canonicalize(crate::resolve::find_root(workspace)?).ok();
    if !is_workspace && resolved.as_deref() != Some(root) {
        return Err(format!("{} is neither a workspace root nor the package root cargo resolves: refused", root.display()));
    }
    Ok(Some(real))
}

/// Folder names compare without case on Windows (its file system's rule), exactly elsewhere.
fn same_name(n: Option<&std::ffi::OsStr>, want: &str) -> bool {
    let Some(n) = n.and_then(|n| n.to_str()) else { return false };
    if cfg!(windows) { n.eq_ignore_ascii_case(want) } else { n == want }
}

#[derive(Default)]
struct Removal {
    files: u64,
    bytes: u64,
    skipped: Vec<(PathBuf, String)>,
    removed: Vec<PathBuf>,
}

impl Removal {
    /// Removes one file (or link) if it exists.
    fn file(&mut self, p: &Path) {
        let Ok(m) = std::fs::symlink_metadata(p) else { return };
        match std::fs::remove_file(p) {
            Ok(()) => {
                self.files += 1;
                self.bytes += if m.file_type().is_file() { m.len() } else { 0 };
                self.removed.push(p.to_path_buf());
            }
            Err(e) => self.skipped.push((p.to_path_buf(), e.to_string())),
        }
    }

    /// Removes the folder `d` and everything in it, depth first, links removed and not followed.
    fn tree(&mut self, d: &Path) {
        let entries: Vec<_> = match std::fs::read_dir(d) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(e) => {
                self.skipped.push((d.to_path_buf(), e.to_string()));
                return;
            }
        };
        for p in entries {
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_dir() => self.tree(&p),
                Ok(m) => match std::fs::remove_file(&p) {
                    Ok(()) => {
                        self.files += 1;
                        self.bytes += if m.file_type().is_file() { m.len() } else { 0 };
                    }
                    Err(e) => self.skipped.push((p, e.to_string())),
                },
                Err(_) => {}
            }
        }
        if let Err(e) = std::fs::remove_dir(d) {
            // not empty: a skipped file below it is reported already
            if !self.skipped.iter().any(|(s, _)| s.starts_with(d)) {
                self.skipped.push((d.to_path_buf(), e.to_string()));
            }
        }
    }

    fn report(&self, dir: &Path, package: Option<&str>) {
        match package {
            Some(p) => {
                for f in &self.removed {
                    println!("removed {}", f.display());
                }
                println!("makepad clean -p {p}: removed {} files, {} bytes ({})", self.files, self.bytes, human(self.bytes));
            }
            None => println!("makepad clean: removed {} ({} files, {} bytes, {})", dir.display(), self.files, self.bytes, human(self.bytes)),
        }
        if !self.skipped.is_empty() {
            println!("makepad clean: {} in use (open or mapped by a running app or makepad run), skipped; they go at the next start (stale store generations) or the next clean:", self.skipped.len());
            for (p, e) in &self.skipped {
                println!("  {} ({e})", p.display());
            }
        }
    }
}

fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("relax-clean-{}-{}-{}", tag, std::process::id(), crate::clock::wall_ns()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn w(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn clean_rules() {
        isolated_test!(clean_rules);
        // the rules for an unset target dir: a cargo run of this test may have one set
        // (CARGO_TARGET_DIR is cargo's, and the child is this test alone)
        std::env::remove_var("CARGO_TARGET_DIR");
        std::env::remove_var("CARGO_BUILD_TARGET_DIR");
        let ws = tmp("ws");
        let ws_s = ws.display().to_string();
        w(&ws.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n");
        w(&ws.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\n");
        w(&ws.join("target/release/keep.rlib"), "cargo's");
        w(&ws.join("target/makepad-run/cache/core-0123456789abcdef.rcs"), "0123456789");
        w(&ws.join("target/makepad-run/startup/app.startup"), "abc");

        // under MAKEPAD_RUN_CACHE_DIR: refused
        assert!(checked_dir(&ws_s, true).unwrap_err().contains("MAKEPAD_RUN_CACHE_DIR"));

        // no Cargo.toml at or above the folder: refused, nothing touched
        let bare = tmp("bare");
        w(&bare.join("target/makepad-run/x"), "x");
        assert!(checked_dir(&bare.display().to_string(), false).is_err());
        assert!(bare.join("target/makepad-run/x").exists());

        // a package below a workspace that does not list it is its own root (cargo's rule);
        // a folder named target/makepad-run2 is never cleaned: only `makepad-run` is ever returned
        let solo = tmp("solo");
        w(&solo.join("Cargo.toml"), "[package]\nname = \"solo\"\nversion = \"0.1.0\"\n");
        w(&solo.join("target/makepad-run2/x"), "x");
        assert_eq!(checked_dir(&solo.display().to_string(), false).unwrap(), None);
        w(&solo.join("target/makepad-run/x"), "x");
        assert_eq!(checked_dir(&solo.display().to_string(), false).unwrap(), Some(std::fs::canonicalize(solo.join("target/makepad-run")).unwrap()));

        // a target dir set by cargo config (as CARGO_TARGET_DIR would): exactly `<it>/relax`
        let custom = tmp("custom");
        w(&custom.join("makepad-run/x"), "x");
        let cfg = tmp("cfg");
        w(&cfg.join("Cargo.toml"), "[workspace]\n");
        w(&cfg.join(".cargo/config.toml"), &format!("[build]\ntarget-dir = \"{}\"\n", custom.display()));
        assert_eq!(checked_dir(&cfg.display().to_string(), false).unwrap(), Some(std::fs::canonicalize(custom.join("makepad-run")).unwrap()));

        // a relax dir that is a link elsewhere: refused
        #[cfg(unix)]
        {
            let elsewhere = tmp("elsewhere");
            let linked = tmp("linked");
            w(&linked.join("Cargo.toml"), "[workspace]\n");
            std::fs::create_dir_all(linked.join("target")).unwrap();
            std::os::unix::fs::symlink(&elsewhere, linked.join("target/makepad-run")).unwrap();
            assert!(checked_dir(&linked.display().to_string(), false).is_err());
            assert!(elsewhere.exists());
            std::fs::remove_file(linked.join("target/makepad-run")).unwrap();
            std::fs::remove_dir_all(&linked).unwrap();
            std::fs::remove_dir(&elsewhere).unwrap();
        }

        // the workspace's own, from a member folder: exactly target/makepad-run goes, bytes counted
        let dir = checked_dir(&ws.join("app").display().to_string(), false).unwrap().unwrap();
        assert_eq!(dir, std::fs::canonicalize(ws.join("target/makepad-run")).unwrap());
        let mut r = Removal::default();
        r.tree(&dir);
        assert_eq!((r.files, r.bytes), (2, 13));
        assert!(r.skipped.is_empty());
        assert!(!ws.join("target/makepad-run").exists());
        assert!(ws.join("target/release/keep.rlib").is_file());
        assert_eq!(checked_dir(&ws_s, false).unwrap(), None);

        for d in [&ws, &bare, &solo, &custom, &cfg] {
            std::fs::remove_dir_all(d).unwrap();
        }
    }

    /// A folder whose name is not `makepad-run` under `target` is never what checked_dir returns, and a
    /// file that cannot be removed is skipped and reported, the rest removed.
    #[test]
    fn skipped_files_are_reported() {
        assert!(!same_name(Some(std::ffi::OsStr::new("makepad-run2")), "makepad-run"));
        assert!(!same_name(Some(std::ffi::OsStr::new("tmp")), "target"));
        let d = tmp("skip");
        w(&d.join("a/x"), "12345");
        w(&d.join("b/y"), "1");
        // a folder that refuses removal of its entries (unix: no write permission); on Windows
        // an open/mapped file fails the same way
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(d.join("a"), std::fs::Permissions::from_mode(0o500)).unwrap();
        }
        let mut r = Removal::default();
        r.tree(&d);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(r.files, 1);
            assert!(r.skipped.iter().any(|(p, _)| p.ends_with("a/x")));
            assert!(d.join("a/x").exists() && !d.join("b").exists());
            std::fs::set_permissions(d.join("a"), std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::remove_dir_all(&d).unwrap();
        }
    }
}
