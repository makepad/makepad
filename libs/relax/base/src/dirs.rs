//! Where makepad run keeps what it writes: like cargo's build output, everything for a workspace
//! lives in `<target dir>/relax` (the "run dir"), where the target dir is what `cargo metadata`
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
//! RELAX_CACHE_DIR replaces the whole run dir: for tests and measurement only. A run
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
    key_of(anchors())
}

fn key_of(anchors: &[(&'static str, String)]) -> u128 {
    let mut h = crate::sip::Sip128::new();
    h.str("relax-anchors");
    for (tag, dir) in anchors { h.str(tag); h.str(dir); }
    h.finish128()
}

/// The anchors a code cache is opened against, held as a value (two checkouts in one test are two
/// values): the process's (`run`, `cargo`, `ws`), and where the sources no checkout owns lie --
/// the cargo home, and std's sources when no checkout anchor holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchors {
    /// (tag, absolute normalized dir), as anchors() has them
    dirs: Vec<(&'static str, String)>,
    /// std's sources (absolute, normalized)
    std: String,
}

impl Anchors {
    /// This process's: anchors() and `std`, std's source folder.
    pub fn current(std: &str) -> Anchors {
        Anchors { dirs: anchors().to_vec(), std: absolute_dir(std) }
    }

    /// The anchors `dirs` (each made absolute) with std at `std`.
    pub fn of(dirs: &[(&'static str, &str)], std: &str) -> Anchors {
        Anchors { dirs: dirs.iter().map(|(tag, dir)| (*tag, absolute_dir(dir))).collect(), std: absolute_dir(std) }
    }

    /// anchors_key() of these anchors.
    pub fn key(&self) -> u128 {
        key_of(&self.dirs)
    }

    /// The anchors: (tag, absolute normalized folder).
    pub fn dirs(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.dirs.iter().map(|(tag, dir)| (*tag, dir.as_str()))
    }

    /// `path` in its anchored spelling under these anchors (as `anchored` spells it). None: it lies
    /// under none of them.
    pub fn anchored(&self, path: &str) -> Option<String> {
        anchor_in(path, &self.dirs)
    }

    /// The path an anchored spelling names under these anchors: its anchor's folder, then the
    /// rest. Any other path as it is. None: the marker of an anchor these do not have.
    pub fn unanchored<'a>(&self, path: &'a str) -> Option<std::borrow::Cow<'a, str>> {
        let Some(marked) = path.strip_prefix('\0') else { return Some(std::borrow::Cow::Borrowed(path)) };
        let (tag, rel) = marked.split_once('\0')?;
        let dir = self.dir_of(tag)?;
        Some(std::borrow::Cow::Owned(if rel.is_empty() { dir.to_string() } else { format!("{}/{rel}", dir.trim_end_matches('/')) }))
    }

    /// `text` with each anchor's marker in it (`\0tag\0`, which an anchored path's rest follows)
    /// read as that anchor's folder and a `/`. None: a NUL that starts no marker of these anchors.
    pub fn unanchored_text<'a>(&self, text: &'a str) -> Option<std::borrow::Cow<'a, str>> {
        if !text.contains('\0') { return Some(std::borrow::Cow::Borrowed(text)); }
        let mut out = String::with_capacity(text.len() + 64);
        let mut rest = text;
        while let Some(i) = rest.find('\0') {
            out.push_str(&rest[..i]);
            let (tag, after) = rest[i + 1..].split_once('\0')?;
            out.push_str(self.dir_of(tag)?.trim_end_matches('/'));
            out.push('/');
            rest = after;
        }
        out.push_str(rest);
        Some(std::borrow::Cow::Owned(out))
    }

    fn dir_of(&self, tag: &str) -> Option<&str> {
        self.dirs.iter().find(|(t, _)| *t == tag).map(|(_, dir)| dir.as_str())
    }

    /// The key of the places no checkout owns (the cargo home, std's folder): what a sealed
    /// crate's code is keyed by in place of key(), the same in every checkout over them.
    pub fn machine_key(&self) -> u128 {
        let mut h = crate::sip::Sip128::new();
        h.str("relax-sealed-anchors");
        for (tag, dir) in self.dirs.iter().filter(|(tag, _)| *tag == "cargo") { h.str(tag); h.str(dir); }
        h.str("std");
        h.str(&self.std);
        h.finish128()
    }

    /// The places no checkout owns (the cargo home, std's folder), absolute: where a late input of
    /// a shared code store's ledger may lie.
    pub fn machine_dirs(&self) -> impl Iterator<Item = &str> {
        self.dirs.iter().filter(|(tag, _)| *tag == "cargo").map(|(_, dir)| dir.as_str()).chain(std::iter::once(self.std.as_str()))
    }

    /// The folders of sources no checkout owns, absolute: std's sources and the cargo home's
    /// registry and git folders, each unless it lies in a folder of the checkout (its paths are
    /// then the checkout's). A path inside one is that folder's, whatever its name shares with
    /// the checkout's (a checkout `.../s` beside std at `.../std`).
    pub fn shared_roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self.dirs.iter().filter(|(tag, _)| *tag == "cargo")
            .flat_map(|(_, dir)| ["registry", "git"].map(|sub| format!("{}/{sub}", dir.trim_end_matches('/'))))
            .chain(std::iter::once(self.std.clone()))
            .collect();
        roots.retain(|r| !self.checkout_dirs().any(|dir| crate::vfs::is_under_normalized(r, dir)));
        roots
    }

    /// The checkout's folders (the workspace, the run dir), absolute: text a sealed entry never
    /// holds.
    pub fn checkout_dirs(&self) -> impl Iterator<Item = &str> {
        self.dirs.iter().filter(|(tag, _)| matches!(*tag, "ws" | "run")).map(|(_, dir)| dir.as_str())
    }

    /// Whether `path` is a source no checkout owns: under the cargo home or std's folder, and in
    /// no folder of the checkout, the workspace or the run dir (a cargo home inside the workspace
    /// is the checkout's, and so are packages downloaded into the run dir).
    pub fn sealed_source(&self, path: &str) -> bool {
        let p = crate::vfs::normalize_path(path);
        let under = |tags: &[&str]| self.dirs.iter().any(|(tag, dir)| tags.contains(tag) && crate::vfs::is_under_normalized(&p, dir));
        (under(&["cargo"]) || crate::vfs::is_under_normalized(&p, &self.std)) && !under(&["ws", "run"])
    }
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

/// The workspace: `--workspace-dir` (`set_workspace_dir`), else RELAX_WORKSPACE (a git run's
/// virtual folder, vfs.rs; it does not exist on disk, so it cannot be the current folder), else
/// the current folder.
pub fn workspace_dir() -> String {
    if let Some(w) = SET_WORKSPACE.get() {
        return w.clone();
    }
    if let Some(w) = std::env::var_os("RELAX_WORKSPACE").filter(|w| !w.is_empty()) {
        return w.to_string_lossy().into_owned();
    }
    std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| ".".to_string())
}

/// The loader's executable, for a spawn of the loader by itself (a build script's run, cargo's
/// wrapper, a check, the check server, a test target's run): RELAX_LOADER, which the runner sets at
/// its start, else this process's executable. Never a run's own executable (relax-compiler
/// selfexe.rs): a launch of that is its program again, so a run started from one (a test's run of
/// itself) ran its build scripts as its test program, which waited for the build script lock its
/// parent held (11-13), and a program that relaunched itself with a cleared environment exported
/// its link as the loader, whose every spawn of "the loader" ran the program again, without end.
/// RELAX_LOADER naming a run's executable is an error, not a spawn; this process running from one
/// spawns the loader recorded beside it (`RUN_LOADER`).
pub fn loader_exe() -> std::io::Result<PathBuf> {
    match std::env::var_os("RELAX_LOADER") {
        Some(p) if !p.is_empty() => {
            let p = PathBuf::from(p);
            if is_run_link(&p) {
                return Err(std::io::Error::other(format!("RELAX_LOADER names {}, a run's own executable (a launch of it runs that run's program), not the loader", p.display())));
            }
            Ok(p)
        }
        _ => {
            let exe = std::env::current_exe()?;
            if !is_run_link(&exe) {
                return Ok(exe);
            }
            recorded_loader(&exe).ok_or_else(|| std::io::Error::other(format!("{} is a run's own executable and no loader is recorded beside it (RELAX_LOADER is not set)", exe.display())))
        }
    }
}

/// The files beside a run's own executable (relax-compiler selfexe.rs): names no link has (a link
/// keeps a name's letters, digits, `-`, `_` and `.`). The folder of the runs of the links there
/// (`+run/<pid>`: each run's command line, RELAX_SELF_EXEC's words, and where a launch of the link
/// by a shell finds its build environment, progenv `build_wire`; on Unix the descriptor of a file
/// without a name, never the environment itself), the loader the links were made of, the folder
/// of the links a process starts runs from, and on Windows the command lines of a test's package
/// binaries.
pub const RUN_ENTRIES: &str = "+run";
pub const RUN_LOADER: &str = "+loader";
pub const LAUNCH_DIR: &str = "+launch";
pub const BIN_LINES: &str = "+bin";

/// Whether `exe` is a run's own executable (its runs' folder beside it), a link a run was started
/// from (in a `+launch` folder), or a package binary above a test's `deps/` (its line in `+bin`):
/// none is the loader's own path. A launch of a run's link runs that run's program, as one of a
/// package binary runs that binary.
pub fn is_run_link(exe: &std::path::Path) -> bool {
    exe.parent().is_some_and(|d| {
        d.join(RUN_ENTRIES).is_dir() || d.file_name().is_some_and(|n| n == LAUNCH_DIR) || exe.file_name().is_some_and(|f| d.join(BIN_LINES).join(f).is_file())
    })
}

/// The loader a run's link (or a launch link) was made of, as its maker wrote it beside it; None
/// when none is there or it names a run's link itself.
pub fn recorded_loader(exe: &std::path::Path) -> Option<PathBuf> {
    let bytes = std::fs::read(exe.parent()?.join(RUN_LOADER)).ok()?;
    #[cfg(unix)]
    let loader = PathBuf::from(<std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(bytes));
    #[cfg(not(unix))]
    let loader = PathBuf::from(String::from_utf8(bytes).ok()?);
    (loader.is_file() && !is_run_link(&loader)).then_some(loader)
}

/// The cargo of a build, as cargo names itself to what it builds (CARGO, an absolute path): the one
/// this process was started with when that names a cargo by an absolute path (a run under cargo:
/// its toolchain's), else the first `cargo` on PATH; None without one.
pub fn cargo_exe() -> Option<String> {
    static CARGO: OnceLock<Option<String>> = OnceLock::new();
    CARGO
        .get_or_init(|| {
            let cargo = |p: &Path| p.is_absolute() && p.is_file() && p.file_stem().is_some_and(|s| s == "cargo");
            if let Some(c) = std::env::var_os("CARGO").map(PathBuf::from).filter(|c| cargo(c)) {
                return Some(c.display().to_string());
            }
            let name = if cfg!(windows) { "cargo.exe" } else { "cargo" };
            std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|c| cargo(c)).map(|c| c.display().to_string())
        })
        .clone()
}

/// This process's relax dir: the handed-over one, else RELAX_CACHE_DIR, else the workspace's
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

/// RELAX_CACHE_DIR when set (tests and measurement).
pub fn override_dir() -> Option<PathBuf> {
    std::env::var_os("RELAX_CACHE_DIR").filter(|d| !d.is_empty()).map(|dir| {
        let dir = PathBuf::from(dir);
        std::path::absolute(&dir).unwrap_or(dir)
    })
}

/// The relax dir of the workspace holding `start` (no override): `<cargo target dir>/relax`; a
/// folder with no Cargo.toml at or above it: `<start>/target/relax`; a virtual (git store)
/// workspace: `<relax home>/target/<8 hex>`.
pub fn relax_dir_for(start: &str) -> PathBuf {
    if crate::vfs::is_virtual(start) {
        return virtual_dir(&crate::vfs::outer_root(start).unwrap_or_else(|| crate::vfs::normalize_path(start)));
    }
    let target = crate::resolve::target_dir(start).unwrap_or_else(|_| format!("{}/target", crate::vfs::normalize_path(start)));
    Path::new(&target).join("relax")
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

/// The volume holding `path`: (bytes this user may still write there, the volume's size). None
/// where the system does not say. (The calls as platform/src/storage.rs makes them.)
pub fn volume_space(path: &Path) -> Option<(u64, u64)> {
    volume::space(path)
}

#[cfg(all(unix, target_pointer_width = "64", not(target_vendor = "apple")))]
mod volume {
    use std::os::unix::ffi::OsStrExt;
    /// `struct statvfs` (64-bit Linux and the BSDs' layout), with room to spare.
    #[repr(C)]
    struct StatVfs {
        block_size: usize,
        fragment_size: usize,
        blocks: u64,
        blocks_free: u64,
        blocks_available: u64,
        files: u64,
        files_free: u64,
        files_available: u64,
        filesystem_id: usize,
        flags: usize,
        name_max: usize,
        spare: [u64; 32],
    }
    unsafe extern "C" {
        fn statvfs(path: *const std::ffi::c_char, out: *mut StatVfs) -> std::ffi::c_int;
    }
    pub(super) fn space(path: &std::path::Path) -> Option<(u64, u64)> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: plain data, filled by the call
        let mut s: StatVfs = unsafe { std::mem::zeroed() };
        if unsafe { statvfs(path.as_ptr(), &mut s) } != 0 {
            return None;
        }
        // the counts are in fragments (f_frsize); a system that leaves it 0 counts in blocks
        let unit = if s.fragment_size > 0 { s.fragment_size } else { s.block_size } as u64;
        Some((unit.saturating_mul(s.blocks_available), unit.saturating_mul(s.blocks)))
    }
}

#[cfg(target_vendor = "apple")]
mod volume {
    use std::os::unix::ffi::OsStrExt;
    /// `struct statfs`, the 64-bit inode layout.
    #[repr(C)]
    struct StatFs {
        block_size: u32,
        io_size: i32,
        blocks: u64,
        blocks_free: u64,
        blocks_available: u64,
        files: u64,
        files_free: u64,
        filesystem_id: [i32; 2],
        owner: u32,
        filesystem_type: u32,
        flags: u32,
        filesystem_subtype: u32,
        filesystem_type_name: [u8; 16],
        mount_on_name: [u8; 1024],
        mount_from_name: [u8; 1024],
        flags_ext: u32,
        reserved: [u32; 7],
    }
    unsafe extern "C" {
        // on x86_64 the plain name is the 32-bit inode layout's
        #[cfg_attr(target_arch = "x86_64", link_name = "statfs$INODE64")]
        fn statfs(path: *const std::ffi::c_char, out: *mut StatFs) -> std::ffi::c_int;
    }
    pub(super) fn space(path: &std::path::Path) -> Option<(u64, u64)> {
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: plain data, filled by the call
        let mut s: StatFs = unsafe { std::mem::zeroed() };
        if unsafe { statfs(path.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let unit = s.block_size as u64;
        Some((unit.saturating_mul(s.blocks_available), unit.saturating_mul(s.blocks)))
    }
}

#[cfg(windows)]
mod volume {
    use std::os::windows::ffi::OsStrExt;
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(directory: *const u16, available: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    }
    pub(super) fn space(path: &std::path::Path) -> Option<(u64, u64)> {
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut available, &mut total, &mut free) } == 0 {
            return None;
        }
        Some((available, total))
    }
}

#[cfg(not(any(windows, target_vendor = "apple", all(unix, target_pointer_width = "64"))))]
mod volume {
    pub(super) fn space(_path: &std::path::Path) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    /// The volume query reads the system's struct right: a folder's volume has a size, at most
    /// all of it free, and a missing path has none.
    #[test]
    fn a_folders_volume_has_a_size_and_at_most_all_of_it_free() {
        if cfg!(any(windows, all(unix, target_pointer_width = "64"))) {
            let (free, size) = super::volume_space(&std::env::temp_dir()).expect("the temp folder's volume");
            assert!(size > 0 && free <= size, "free {free} of {size} bytes");
            assert_eq!(super::volume_space(std::path::Path::new("/no/such/folder/here")), None);
        }
    }

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
        // read back under another checkout's anchors: its own folders
        let other = super::Anchors::of(&[("ws", "/b/ws"), ("run", "/b/ws/target/run")], "/s");
        assert_eq!(other.unanchored("\0ws\0src/f.rs").as_deref(), Some("/b/ws/src/f.rs"), "a path");
        assert_eq!(other.unanchored("\0run\0").as_deref(), Some("/b/ws/target/run"), "an anchor's own folder");
        assert_eq!(other.unanchored("/b/f.rs").as_deref(), Some("/b/f.rs"), "an unanchored path");
        assert_eq!(other.unanchored("\0cargo\0f.rs"), None, "an anchor it lacks");
        assert_eq!(other.unanchored_text("\0ws\0f.rs:3:9: warning: x\n  --> \0run\0g.rs:1:2").as_deref(), Some("/b/ws/f.rs:3:9: warning: x\n  --> /b/ws/target/run/g.rs:1:2"), "a text");
        assert_eq!((other.unanchored_text("a \0 b"), other.unanchored_text("\0cargo\0f.rs")), (None, None), "a NUL that starts no marker of its anchors");
    }

    /// A run's link (its runs' folder beside it) and a launch link are no loader; each names the
    /// loader it was made of, and a record that names a run's link is none.
    #[test]
    fn a_runs_link_names_the_loader_it_was_made_of() {
        use super::{is_run_link, recorded_loader, LAUNCH_DIR, RUN_ENTRIES, RUN_LOADER};
        let root = std::env::temp_dir().join(format!("relax-run-link-{}", std::process::id()));
        let file = |p: &std::path::Path, text: &[u8]| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        let loader = root.join("bin/relax");
        let (run, again, launch) = (root.join("exe/k1/run/app"), root.join("exe/k2/run/app"), root.join("exe/k1").join(LAUNCH_DIR).join("app"));
        file(&loader, b"");
        for link in [&run, &again, &launch] {
            file(link, b"");
        }
        std::fs::create_dir_all(root.join("exe/k1/run").join(RUN_ENTRIES)).unwrap();
        std::fs::create_dir_all(root.join("exe/k2/run").join(RUN_ENTRIES)).unwrap();
        file(&root.join("exe/k1/run").join(RUN_LOADER), loader.as_os_str().as_encoded_bytes());
        file(&root.join("exe/k1").join(LAUNCH_DIR).join(RUN_LOADER), loader.as_os_str().as_encoded_bytes());
        file(&root.join("exe/k2/run").join(RUN_LOADER), run.as_os_str().as_encoded_bytes());
        let got = [is_run_link(&loader), is_run_link(&run), is_run_link(&launch)];
        let recorded = [recorded_loader(&run), recorded_loader(&launch), recorded_loader(&again), recorded_loader(&loader)];
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(got, [false, true, true], "which is a link: loader, run's, launch");
        assert_eq!(recorded, [Some(loader.clone()), Some(loader), None, None], "the recorded loader: run's, launch, one naming a run's link, the loader's own");
    }
}
