//! Jailed per-app file storage for Splash isolates.
//!
//! Mini-apps get an OS-style private data directory — the same model as an
//! Android app's internal storage or an iOS app container: the app sees a
//! filesystem rooted at "/", and that root IS its own sandbox directory. The
//! host assigns the real directory per isolate (`Splash::set_sandbox_dir`);
//! script code can never observe or influence the real location.
//!
//! Registered in isolates as `mod.fs` — deliberately shadowing the stripped
//! real fs module, so inside an app "the filesystem" simply *is* the jail:
//!
//! ```text
//! fs.write("/notes.json", text)     fs.read("/notes.json")
//! fs.exists(path)  fs.remove(path)  fs.mkdir(path)  fs.list(path)
//! fs.append(path, text)
//! ```
//!
//! Containment (all enforced here, in the host layer, not in script):
//! - Paths are resolved LEXICALLY against the root: `..` above the root, NUL
//!   bytes, over-long names/depth are errors before any I/O happens. Leading
//!   `/` means "the app's root", not the host filesystem.
//! - The per-VM root lives in Rust state keyed by the isolate's heap — script
//!   code has no handle to read or overwrite it.
//! - Symlink defense in depth: no API here can create links, and every
//!   existing path component under the root is verified non-symlink before
//!   use, so even a link smuggled into the directory can't redirect I/O out.
//! - Quotas: per-file and whole-jail size caps, entry-count cap.
//! - No isolate root configured (previews, validation without one) → every
//!   call errors with "storage not available".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::makepad_draw::makepad_platform::makepad_script;
use crate::widget_async::SplashVmId;
use makepad_script::*;

/// Largest single file the jail accepts (write or append result).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Largest total jail size.
pub const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
/// Most entries (files + dirs) a jail may hold.
pub const MAX_ENTRIES: usize = 256;
/// Deepest nesting and longest single path component.
const MAX_DEPTH: usize = 16;
const MAX_NAME: usize = 128;

std::thread_local! {
    /// heap-key → sandbox root for every live isolate that has one. Thread
    /// local like the isolate registry itself (isolates are UI-thread-owned).
    static SANDBOX_ROOTS: std::cell::RefCell<HashMap<usize, PathBuf>> =
        std::cell::RefCell::new(HashMap::new());
}

/// Assigns (or clears) the sandbox root for an isolate, keyed by its heap.
pub(crate) fn set_root_for_heap(heap_key: usize, root: Option<PathBuf>) {
    SANDBOX_ROOTS.with(|r| {
        let mut r = r.borrow_mut();
        match root {
            Some(root) => {
                r.insert(heap_key, root);
            }
            None => {
                r.remove(&heap_key);
            }
        }
    });
}

/// Drops roots for reclaimed isolates (called from the isolate GC).
pub(crate) fn gc_roots(dead_heaps: &[usize]) {
    SANDBOX_ROOTS.with(|r| {
        let mut r = r.borrow_mut();
        for heap in dead_heaps {
            r.remove(heap);
        }
    });
}

fn root_for_vm(vm: &ScriptVm) -> Option<PathBuf> {
    let heap_key = vm.bx.heap.heap_key();
    SANDBOX_ROOTS.with(|r| r.borrow().get(&heap_key).cloned())
}

/// Resolves an app-visible path to a real path inside `root`, purely
/// lexically — no filesystem access. `..` that would climb above the root is
/// an error (not clamped: a climbing path is always an app bug or an escape
/// attempt, and silently retargeting it would mask both).
pub fn resolve_jailed(root: &Path, virtual_path: &str) -> Result<PathBuf, String> {
    if virtual_path.contains('\0') {
        return Err("invalid path".to_string());
    }
    let mut parts: Vec<&str> = Vec::new();
    for comp in virtual_path.split(['/', '\\']) {
        match comp {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("path escapes the app's storage".to_string());
                }
            }
            name => {
                if name.len() > MAX_NAME {
                    return Err("path component too long".to_string());
                }
                parts.push(name);
            }
        }
    }
    if parts.len() > MAX_DEPTH {
        return Err("path too deep".to_string());
    }
    let mut real = root.to_path_buf();
    for p in parts {
        real.push(p);
    }
    Ok(real)
}

/// Symlink defense: every EXISTING component from `root` down to `real` must
/// be a plain file/dir. The API can't create links, but if one ever appears
/// in the jail (a bug elsewhere, a hostile unpacker later), it must not
/// redirect I/O outside.
fn verify_no_symlinks(root: &Path, real: &Path) -> Result<(), String> {
    let mut cursor = root.to_path_buf();
    let rel = real.strip_prefix(root).map_err(|_| "path escapes the app's storage")?;
    for comp in rel.components() {
        cursor.push(comp);
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("path escapes the app's storage".to_string());
            }
            _ => {}
        }
    }
    Ok(())
}

/// Current jail usage: (total bytes, entry count). Small trees by
/// construction (entry cap), so a full walk per write is fine.
fn jail_usage(root: &Path) -> (u64, usize) {
    fn walk(dir: &Path, bytes: &mut u64, entries: &mut usize) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            *entries += 1;
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&entry.path(), bytes, entries);
            } else {
                *bytes += meta.len();
            }
        }
    }
    let mut bytes = 0;
    let mut entries = 0;
    walk(root, &mut bytes, &mut entries);
    (bytes, entries)
}

/// How many path components between `root` and `real` don't exist yet — what
/// `create_dir_all` would newly materialize (plus the leaf if absent). Used
/// to charge directory creation against the entry cap, so `mkdir` and deep
/// writes can't sprawl inodes past `MAX_ENTRIES` unbounded.
fn missing_entries(root: &Path, real: &Path) -> usize {
    let mut n = 0;
    let mut cursor = real.to_path_buf();
    while cursor != *root && !cursor.exists() {
        n += 1;
        if !cursor.pop() {
            break;
        }
    }
    n
}

/// A resolved, symlink-checked target ready for I/O, or a script error string.
fn target(vm: &mut ScriptVm, path_value: ScriptValue) -> Result<(PathBuf, PathBuf), String> {
    let Some(root) = root_for_vm(vm) else {
        return Err("storage not available in this context".to_string());
    };
    let mut virtual_path = None;
    // (string_with yields nothing for non-strings.)
    vm.string_with(path_value, |_vm, s| {
        virtual_path = Some(s.to_string());
    });
    let Some(virtual_path) = virtual_path else {
        return Err("path must be a string".to_string());
    };
    let real = resolve_jailed(&root, &virtual_path)?;
    verify_no_symlinks(&root, &real)?;
    Ok((root, real))
}

/// Registers the jailed `fs` module into an isolate VM.
pub fn script_mod(vm: &mut ScriptVm) {
    let fs = vm.new_module(id!(fs));

    vm.add_method(fs, id_lut!(read), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        match target(vm, path) {
            Ok((_root, real)) => match std::fs::read_to_string(&real) {
                Ok(text) => vm.new_string_with(|_vm, s| s.push_str(&text)).into(),
                Err(_) => script_err_io!(vm.trap(), "file not found"),
            },
            Err(e) => script_err_io!(vm.trap(), "{}", e),
        }
    });

    for (sym, append) in [(id_lut!(write), false), (id_lut!(append), true)] {
        vm.add_method(
            fs,
            sym,
            script_args_def!(path = NIL, data = NIL),
            move |vm, args| {
                let path = script_value!(vm, args.path);
                let data = script_value!(vm, args.data);
                let (root, real) = match target(vm, path) {
                    Ok(t) => t,
                    Err(e) => return script_err_io!(vm.trap(), "{}", e),
                };
                // The root is a directory; writing "/" (or "" / "/x/..") must
                // not reach create_dir_all(root.parent()) below, which lands
                // OUTSIDE the jail. Reject it like `remove` does.
                if real == root {
                    return script_err_io!(vm.trap(), "cannot write the storage root");
                }
                let mut text = None;
                vm.string_with(data, |_vm, s| text = Some(s.to_string()));
                let Some(text) = text else {
                    return script_err_io!(vm.trap(), "data must be a string");
                };
                // Quotas, before any bytes land.
                let existing = std::fs::metadata(&real).map(|m| m.len()).unwrap_or(0);
                let new_len = if append {
                    existing + text.len() as u64
                } else {
                    text.len() as u64
                };
                if new_len > MAX_FILE_BYTES {
                    return script_err_io!(vm.trap(), "file too large");
                }
                let (total, entries) = jail_usage(&root);
                if total - existing.min(total) + new_len > MAX_TOTAL_BYTES {
                    return script_err_io!(vm.trap(), "app storage is full");
                }
                // Charge the file AND any parent dirs create_dir_all would make,
                // so a deep write can't overshoot the entry cap.
                if entries + missing_entries(&root, &real) > MAX_ENTRIES {
                    return script_err_io!(vm.trap(), "too many files");
                }
                if let Some(parent) = real.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let result = if append {
                    use std::io::Write;
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&real)
                        .and_then(|mut f| f.write_all(text.as_bytes()))
                } else {
                    std::fs::write(&real, text.as_bytes())
                };
                match result {
                    Ok(()) => NIL,
                    Err(_) => script_err_io!(vm.trap(), "write failed"),
                }
            },
        );
    }

    vm.add_method(fs, id_lut!(exists), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        match target(vm, path) {
            Ok((_root, real)) => real.exists().into(),
            Err(e) => script_err_io!(vm.trap(), "{}", e),
        }
    });

    vm.add_method(fs, id_lut!(remove), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        match target(vm, path) {
            Ok((root, real)) => {
                if real == root {
                    return script_err_io!(vm.trap(), "cannot remove the storage root");
                }
                let result = match std::fs::metadata(&real) {
                    Ok(m) if m.is_dir() => std::fs::remove_dir(&real),
                    Ok(_) => std::fs::remove_file(&real),
                    Err(e) => Err(e),
                };
                match result {
                    Ok(()) => NIL,
                    Err(_) => script_err_io!(vm.trap(), "remove failed"),
                }
            }
            Err(e) => script_err_io!(vm.trap(), "{}", e),
        }
    });

    vm.add_method(fs, id_lut!(mkdir), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        match target(vm, path) {
            Ok((root, real)) => {
                // Charge new dirs against the entry cap — otherwise mkdir is an
                // unbounded inode-exhaustion hole (it never touched jail_usage).
                let missing = missing_entries(&root, &real);
                if missing > 0 {
                    let (_bytes, entries) = jail_usage(&root);
                    if entries + missing > MAX_ENTRIES {
                        return script_err_io!(vm.trap(), "too many files");
                    }
                }
                match std::fs::create_dir_all(&real) {
                    Ok(()) => NIL,
                    Err(_) => script_err_io!(vm.trap(), "mkdir failed"),
                }
            }
            Err(e) => script_err_io!(vm.trap(), "{}", e),
        }
    });

    // Directory listing: an array of names, directories marked with a
    // trailing "/". Sorted for determinism.
    vm.add_method(fs, id_lut!(list), script_args_def!(path = NIL), |vm, args| {
        let path = script_value!(vm, args.path);
        match target(vm, path) {
            Ok((_root, real)) => {
                let mut names: Vec<String> = Vec::new();
                match std::fs::read_dir(&real) {
                    Ok(read) => {
                        for entry in read.flatten() {
                            let mut name = entry.file_name().to_string_lossy().into_owned();
                            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                                name.push('/');
                            }
                            names.push(name);
                        }
                    }
                    Err(_) => return script_err_io!(vm.trap(), "not a directory"),
                }
                names.sort();
                let array = vm.bx.heap.new_array();
                for name in &names {
                    let name = vm.bx.heap.new_string_from_str(name);
                    vm.bx.heap.array_push_unchecked(array, name);
                }
                array.into()
            }
            Err(e) => script_err_io!(vm.trap(), "{}", e),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jail_resolution_contains_and_rejects() {
        let root = Path::new("/jail/app");
        let ok = |p: &str| resolve_jailed(root, p).unwrap();
        assert_eq!(ok("/notes.json"), root.join("notes.json"));
        assert_eq!(ok("notes.json"), root.join("notes.json"));
        assert_eq!(ok("/a/./b//c"), root.join("a").join("b").join("c"));
        assert_eq!(ok("/a/../b"), root.join("b"));
        assert_eq!(ok("/"), root.to_path_buf());

        // Escapes and abuse are errors, not clamps.
        assert!(resolve_jailed(root, "..").is_err());
        assert!(resolve_jailed(root, "/../etc/passwd").is_err());
        assert!(resolve_jailed(root, "a/../../b").is_err());
        assert!(resolve_jailed(root, "\\..\\..\\x").is_err());
        assert!(resolve_jailed(root, "a\0b").is_err());
        let long = "x".repeat(200);
        assert!(resolve_jailed(root, &long).is_err());
        let deep = (0..20).map(|_| "d").collect::<Vec<_>>().join("/");
        assert!(resolve_jailed(root, &deep).is_err());
    }

    #[test]
    fn symlink_defense_blocks_links() {
        let dir = std::env::temp_dir().join("mp_jail_symlink_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", dir.join("link")).unwrap();
            let real = resolve_jailed(&dir, "/link/passwd").unwrap();
            assert!(verify_no_symlinks(&dir, &real).is_err());
        }
        let real = resolve_jailed(&dir, "/sub/file.txt").unwrap();
        assert!(verify_no_symlinks(&dir, &real).is_ok());
    }

    #[test]
    fn missing_entries_counts_new_components() {
        let dir = std::env::temp_dir().join("mp_jail_missing_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a")).unwrap();
        // a/ exists; a/b and a/b/c.txt do not → 2 new components.
        assert_eq!(missing_entries(&dir, &dir.join("a/b/c.txt")), 2);
        // fully existing → 0.
        std::fs::write(dir.join("a/x.txt"), b"hi").unwrap();
        assert_eq!(missing_entries(&dir, &dir.join("a/x.txt")), 0);
        // the root itself is never counted.
        assert_eq!(missing_entries(&dir, &dir), 0);
    }
}
