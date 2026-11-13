//! The program's own executable under the JIT: a hard link of the loader in this run's own folder,
//! `<run dir>/exe/<pid>/<name>` (`<name>.exe` on Windows), which the program's argv[0] and
//! std::env::current_exe() name. A spawn of that path runs this program again (std
//! process_unix.rs / process_windows.rs `self_exec_args`: RELAX_SELF_EXEC starts with it); a spawn
//! of the loader by any other name (RELAX_LOADER, an installed `makepad`) runs the loader's command
//! line as written, whatever its first word. A hard link, not a symlink or a copy: canonicalize
//! keeps its name, the OS runs it as the loader (the same file, so the same code signature on
//! macOS), and it takes no space beyond a directory entry while the loader file exists. A run dir
//! on another volume than the loader's links a copy of the loader there instead, one per loader
//! file (`link_loader`).
//!
//! The folder also holds the run's command line (`LINE`, RELAX_SELF_EXEC's value), so a launch of
//! the link that did not come through Relax std's spawn (`sh -c "exec $EXE ..."`, a C library's
//! exec) runs this program again too: the loader started by that path finds the line beside
//! itself (`relaunched`), as std's spawn would have put it before the arguments. And the run's
//! build environment (`BUILD`), which std's spawn hands such a run (progenv `BUILD_ENV`): that
//! launch builds with it too, whatever environment the shell gave it, so it is this run's build
//! and its program has the shell's environment (`relaunch_build`, relax issue 172).
//!
//! The folder is laid out as cargo lays out target/<profile>: a test's executable sits in its
//! `deps/` folder and the package's binaries (their CARGO_BIN_EXE_<name> launchers) in the folder
//! itself (`add_bins`), so a test that finds a binary at `current_exe()/../../<name>`, as under
//! cargo, finds it (relax issue 139's second case). On Windows that is `<name>.exe`, a link of the
//! loader whose run's command line is in the folder's `+bin` (`bin_start`).
//!
//! A process gets its program's name from the file it starts from (Linux's comm, macOS's p_comm:
//! what ps, top and a parent lookup show), as a cargo-built program has its own. So whoever starts
//! a run starts it from a link of the loader named as the program (`launch_link`, in the starter's
//! `+launch` folder): `makepad test` its targets' runs, a restart in place the run again. The run
//! started still makes its own link.
//!
//! The folder goes when the run exits (os::proc::exit); a run that did not get there (killed)
//! leaves it, and the next run's `link` removes the folders of processes that are gone. They
//! matter: a link keeps an old loader's blocks alive after cargo replaces the binary.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static OWN_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The folder of the link: OWN_DIR, a test's `deps/` in it
static LINK_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The file name of this run's link
static NAME: OnceLock<String> = OnceLock::new();
/// A process's links that runs start from (`launch_link`), in its folder: a name no link has
const LAUNCH: &str = crate::dirs::LAUNCH_DIR;
/// The loader's copies the links are made of where the loader is on another volume
/// (`link_loader`), beside the processes' folders: a name no process's folder has
const COPIES: &str = "+copy";

/// Writes the loader a link in `dir` was made of beside it (dirs::RUN_LOADER): a process that
/// runs from the link and was not told the loader (RELAX_LOADER: a program that relaunched itself
/// with a cleared environment) spawns that loader, never its own link.
fn record_loader(dir: &Path, loader: &Path) {
    // a link made from another run's link (a program relaunched from it): that link's loader
    let real = if crate::dirs::is_run_link(loader) { crate::dirs::recorded_loader(loader).or_else(|| crate::dirs::loader_exe().ok()) } else { Some(loader.to_path_buf()) };
    if let Some(real) = real {
        let _ = std::fs::write(dir.join(crate::dirs::RUN_LOADER), real.as_os_str().as_encoded_bytes());
    }
}

/// The file name of a program's link: its name's letters, digits, `-`, `_` and `.` (`.exe` added
/// on Windows).
fn file_name(name: &str) -> String {
    let name: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect();
    if cfg!(windows) { format!("{name}.exe") } else { name }
}

/// The name of a run's program, as cargo names its executable: the binary `--bin` names in the
/// run's `flags`, else the package's.
pub fn program_name(flags: &[String], package: &str) -> String {
    flags.iter().position(|f| f == "--bin").and_then(|j| flags.get(j + 1)).cloned()
        .or_else(|| flags.iter().find_map(|f| f.strip_prefix("--bin=").map(str::to_string)))
        .unwrap_or_else(|| Path::new(package).file_stem().map_or_else(|| package.to_string(), |s| s.to_string_lossy().into_owned()))
}

/// `dir` and the folders above it it creates, each owner-only at its mkdir (on Windows they
/// inherit the run dir's owner-only DACL, run.rs own_run_dir): never made, then chmod'ed.
fn private_dirs(dir: &Path) -> std::io::Result<()> {
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(dir)
}

/// This process's link to `loader`, named `name` (a package or binary name), made once; the
/// reason when the run dir's file system cannot hold it (another volume than the loader's).
/// `test`: a test's executable, in the folder's `deps/` as cargo puts it.
pub fn link(loader: &Path, name: &str, test: bool) -> Result<PathBuf, String> {
    let root = crate::dirs::relax_dir().join("exe");
    sweep(&root);
    let dir = root.join(std::process::id().to_string());
    let at = if test { dir.join("deps") } else { dir.clone() };
    let link = at.join(file_name(name));
    let _ = NAME.set(file_name(name));
    // a restart in place: this process again from its launch link, its run's link still there
    if dir.canonicalize().is_ok_and(|d| loader.starts_with(d.join(LAUNCH))) && link.is_file() {
        let _ = OWN_DIR.set(dir);
        let _ = LINK_DIR.set(at);
        return link.canonicalize().map_err(|e| format!("{}: {e}", link.display()));
    }
    // an exec of the program's own executable is this process going on (its pid) in a new image,
    // which runs from the link in this folder: that link is its own executable still (removing
    // the folder took the file it runs from, the new link found no loader, and a spawn of
    // current_exe() failed: not found, 11-13)
    if dir.canonicalize().is_ok_and(|d| loader.starts_with(d)) {
        let _ = OWN_DIR.set(dir);
        let _ = LINK_DIR.set(loader.parent().map_or_else(PathBuf::new, Path::to_path_buf));
        return Ok(loader.to_path_buf());
    }
    // a folder under this pid is a gone process's (pids are reused)
    let _ = std::fs::remove_dir_all(&dir);
    private_dirs(&at).map_err(|e| format!("{}: {e}", at.display()))?;
    let _ = OWN_DIR.set(dir.clone());
    // beside the link, and beside a test's package binaries above its deps/ (add_bins)
    record_loader(&at, loader);
    if at != dir {
        record_loader(&dir, loader);
    }
    let _ = LINK_DIR.set(at);
    link_loader(loader, &link, &root.join(COPIES)).map_err(|e| format!("hard link {} of {}: {e}", link.display(), loader.display()))?;
    link.canonicalize().map_err(|e| format!("{}: {e}", link.display()))
}

/// A hard link of `loader` at `to`. Where the file system cannot hold one because the loader is
/// on another volume (Windows' ERROR_NOT_SAME_DEVICE, Unix's EXDEV), a hard link of the loader's
/// copy in `copies`, a folder on `to`'s volume: a copy made once per loader file (its length and
/// modification time name it), which the links of every later run share. The copies of other
/// loader files go (a run's link keeps the file it runs from, and Windows keeps one in use).
/// Before, a target dir on another drive than relax.exe left the run without its own executable
/// (relax issue 163: "hard link ... (os error 17)").
fn link_loader(loader: &Path, to: &Path, copies: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(loader, to) {
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            let m = std::fs::metadata(loader)?;
            let modified = m.modified()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let name = format!("{:x}-{modified:x}-{}", m.len(), loader.file_name().unwrap_or_default().to_string_lossy());
            let copy = copies.join(&name);
            if !copy.is_file() {
                private_dirs(copies)?;
                let part = copies.join(format!(".{}.{name}", std::process::id()));
                std::fs::copy(loader, &part)?;
                // (another run's copy of the same loader may have got there first)
                if std::fs::rename(&part, &copy).is_err() {
                    let _ = std::fs::remove_file(&part);
                }
                for e in std::fs::read_dir(copies).into_iter().flatten().flatten() {
                    let other = e.file_name().to_string_lossy().into_owned();
                    // a copy being made: `.<pid>.<name>` of a process that is still there
                    let making = other.strip_prefix('.').and_then(|o| o.split('.').next()?.parse::<u32>().ok()).is_some_and(alive);
                    if other != name && !making {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            std::fs::hard_link(&copy, to)
        }
        linked => linked,
    }
}

/// A link of `loader` named as the program `name`, in this process's `+launch` folder, for a run
/// this process starts (or execs, a restart in place) to start from, so its process has the
/// program's name from its start. None where it cannot be made (another volume, Windows, whose
/// processes are not named by an exec): the caller starts `loader`.
///
/// Every start a parent of ours makes goes through such a link, with no exec more (a cold start
/// has no floor): `makepad test` (its targets), a restart in place, devhub's node (a JIT run of a
/// program, devhub lease/exec.rs launch_link) and the app launcher (`app <id>`, tools/run/launcher).
/// A user's `makepad run` / `relax run` in a terminal has no parent of ours: on macOS that run
/// keeps the loader's name; on Linux it renames itself through /proc comm (`name_process`).
#[cfg(unix)]
pub fn launch_link(loader: &Path, name: &str) -> Option<PathBuf> {
    static FRESH: std::sync::Once = std::sync::Once::new();
    let own = crate::dirs::relax_dir().join("exe").join(std::process::id().to_string());
    // a folder under this pid that holds no run of this process is a gone process's
    FRESH.call_once(|| {
        if OWN_DIR.get().is_none() {
            let _ = std::fs::remove_dir_all(&own);
        }
    });
    let dir = own.join(LAUNCH);
    std::fs::create_dir_all(&dir).ok()?;
    record_loader(&dir, loader);
    let path = dir.join(file_name(name));
    if !same_file(&path, loader) {
        // (a restart may start another loader: the link is replaced whole)
        let part = dir.join(format!(".{}.part", file_name(name)));
        let _ = std::fs::remove_file(&part);
        link_loader(loader, &part, &own.with_file_name(COPIES)).ok()?;
        std::fs::rename(&part, &path).ok()?;
    }
    Some(path)
}

#[cfg(not(unix))]
pub fn launch_link(_loader: &Path, _name: &str) -> Option<PathBuf> {
    None
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// What a restart in place execs: `loader` by the run's launch link, so the process keeps its
/// program's name.
pub fn restart_exe(loader: PathBuf) -> PathBuf {
    NAME.get().and_then(|n| launch_link(&loader, n)).unwrap_or(loader)
}

/// Removes this process's `+launch` folder (`makepad test` at its end), with its folder when no
/// run of this process is in it.
pub fn remove_launch() {
    let own = crate::dirs::relax_dir().join("exe").join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(if OWN_DIR.get().is_some() { own.join(LAUNCH) } else { own });
}

/// The kernel's name of this process becomes its own executable's file name, as an exec of that
/// file names a process (`/proc/<pid>/comm`, cut to 15 bytes: what ps, top and a parent lookup
/// show), for a run started from the loader itself (a terminal's `makepad run`); one started from
/// a launch link has it already. It was the loader's (`makepad.jit-…`) while argv[0] and
/// current_exe() named the link, where under cargo all three name the program (makepad-remote's
/// process test, 11-13). macOS has no call that renames a process: there such a run keeps the
/// loader's name.
pub fn name_process(link: &Path) {
    #[cfg(target_os = "linux")]
    if let Some(name) = link.file_name() {
        // any thread of the process may set its leader's name through /proc/<pid>/comm
        let _ = std::fs::write(format!("/proc/{}/comm", std::process::id()), name.as_encoded_bytes());
    }
    #[cfg(not(target_os = "linux"))]
    let _ = link;
}

/// The command line beside the link: a name no link has (`link` keeps a name's letters, digits,
/// `-`, `_` and `.`).
const LINE: &str = crate::dirs::RUN_LINE;

/// The run's build environment beside the link: a name no link has.
const BUILD: &str = crate::dirs::RUN_BUILD;

/// Writes this run's command line (RELAX_SELF_EXEC's words, up to and with `--`) and its build
/// environment beside its link. The folder is the owner's alone (`private_dirs`), as the process's
/// own environment is.
pub fn set_line(words: &[String]) -> Result<(), String> {
    let dir = LINK_DIR.get().ok_or("no link was made")?;
    std::fs::write(dir.join(BUILD), crate::progenv::build_wire()).map_err(|e| format!("{}: {e}", dir.join(BUILD).display()))?;
    std::fs::write(dir.join(LINE), words.join("\u{1f}")).map_err(|e| format!("{}: {e}", dir.join(LINE).display()))
}

/// The build environment of the run whose link this process was launched from (`relaunched`),
/// beside it: what the loader builds with before anything reads the environment (progenv
/// `init_handed`).
pub fn relaunch_build() -> Option<Vec<u8>> {
    std::fs::read(std::env::current_exe().ok()?.parent()?.join(BUILD)).ok()
}

/// The folder's command lines of the package's binaries on Windows (`add_bins`): a name no link has.
const BIN: &str = crate::dirs::BIN_LINES;

/// The package's binaries in this run's folder, as cargo has them in target/<profile> beside a
/// test's `deps/`: `launchers` are the CARGO_BIN_EXE_<name> values, each linked (else copied)
/// under its own file name. A test that finds a binary by that layout ran nothing before. On
/// Windows the launcher is a `.cmd`, and no `.exe` by the binary's name was there, which is
/// where cargo has it (relax PR 153: NotFound): there the binary is `<name>.exe`, a link of the
/// loader, and its run's words (unitgraph.rs, beside the `.cmd`) go in the folder's `+bin`
/// under that name, which the loader started by it takes (`bin_start`). Returns where each one is.
pub fn add_bins(launchers: &[String]) -> Vec<PathBuf> {
    let mut placed = Vec::new();
    let Some(dir) = OWN_DIR.get() else { return placed };
    for l in launchers {
        let from = Path::new(l);
        if cfg!(windows) {
            let (Some(stem), Ok(words)) = (from.file_stem(), std::fs::read(from.with_extension("words"))) else { continue };
            let Some(loader) = words.split(|&b| b == 0x1f).next().map(|w| PathBuf::from(String::from_utf8_lossy(w).into_owned())) else { continue };
            let mut file = stem.to_os_string();
            file.push(".exe");
            let lines = dir.join(BIN);
            if std::fs::create_dir_all(&lines).is_ok() && std::fs::write(lines.join(&file), &words).is_ok() {
                let _ = link_loader(&loader, &dir.join(&file), &dir.with_file_name(COPIES));
                placed.push(dir.join(&file));
            }
            continue;
        }
        let Some(file) = from.file_name() else { continue };
        let to = dir.join(file);
        if !to.exists() && std::fs::hard_link(from, &to).is_err() {
            let _ = std::fs::copy(from, &to);
        }
        placed.push(to);
    }
    placed
}

/// Whether `exe` is a run's own executable (its command line beside it), a link a run was
/// started from (`launch_link`), or a package binary above a test's `deps/` (its line in `+bin`):
/// none is the loader's own path (a launch of a run's link runs that run's program, as one of a
/// package binary runs that binary; a launch link goes with its folder), so the loader running
/// from one spawns itself by RELAX_LOADER (dirs::loader_exe).
pub fn is_link(exe: &Path) -> bool {
    crate::dirs::is_run_link(exe)
}

/// A start of a package binary above a test's `deps/` (Windows, `add_bins`): `args` with the
/// binary's run's words after argv[0], and this process's environment as the binary's `.cmd`
/// gives it: no self-exec line of the test's run, RELAX_ARG0 the path the test started (the
/// program's argv[0], as cargo's binary has it), RELAX_LOADER the loader the words name. None for
/// any other start. The runner's main calls it first, before anything reads the environment or
/// starts a thread.
pub fn bin_start(args: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let exe = std::env::current_exe().ok()?;
    let line = std::fs::read(exe.parent()?.join(BIN).join(exe.file_name()?)).ok()?;
    let mut out: Vec<Vec<u8>> = line.split(|&b| b == 0x1f).map(<[u8]>::to_vec).collect();
    let arg0 = args.first()?.clone();
    unsafe {
        std::env::remove_var("RELAX_SELF_EXEC");
        std::env::remove_var("RELAX_SELF_EXEC_SILENT_ADDED");
        std::env::set_var("RELAX_ARG0", String::from_utf8_lossy(&arg0).into_owned());
        std::env::set_var("RELAX_LOADER", String::from_utf8_lossy(&out[0]).into_owned());
    }
    out[0] = arg0;
    out.extend(args.iter().skip(1).cloned());
    Some(out)
}

/// A launch of a run's link whose arguments do not start with the run's command line (Relax std's
/// spawn and exec put it after argv[0], an exec without the `--silent` at its index 2): `args`
/// with that line after argv[0], as std's spawn gives them. None for any other launch of the
/// loader. (The arguments, not RELAX_SELF_EXEC, tell: a C library's exec passes this process's
/// own environment, which names it, with the program's arguments alone.)
pub fn relaunched(args: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let exe = std::env::current_exe().ok()?;
    let line = std::fs::read(exe.parent()?.join(LINE)).ok()?;
    let mut out: Vec<Vec<u8>> = line.split(|&b| b == 0x1f).map(<[u8]>::to_vec).collect();
    let rest = args.get(1..).unwrap_or_default();
    let mut unsilenced = out[1..].to_vec();
    if unsilenced.get(1).is_some_and(|w| w == b"--silent") {
        unsilenced.remove(1);
    }
    if rest.starts_with(&out[1..]) || rest.starts_with(&unsilenced) {
        return None;
    }
    out[0] = args.first()?.clone();
    out.extend(rest.iter().cloned());
    Some(out)
}

/// Removes this process's folder (os::proc::exit). On Windows the link of a re-run that still
/// runs from it stays; the next run's sweep takes it.
pub fn remove() {
    if let Some(d) = OWN_DIR.get() {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// The folders of processes that are gone.
fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid != std::process::id() && !alive(pid) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    const EPERM: i32 = 1;
    let signalled = unsafe { kill(pid as i32, 0) } == 0;
    signalled || std::io::Error::last_os_error().raw_os_error() == Some(EPERM)
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn GetExitCodeProcess(process: *mut core::ffi::c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            // another user's process is there, only not ours to query
            const ERROR_ACCESS_DENIED: i32 = 5;
            return std::io::Error::last_os_error().raw_os_error() == Some(ERROR_ACCESS_DENIED);
        }
        let mut code = 0u32;
        let running = GetExitCodeProcess(h, &mut code) != 0 && code == STILL_ACTIVE;
        CloseHandle(h);
        running
    }
}

// (Linux: /dev/shm is a volume of its own a test can write to; the Mac and the Windows node have
// none: macOS shows its system and data volumes as one device, and the Windows node has only C:)
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// A loader on another volume than the run dir: each link is a hard link of one copy of it on
    /// the run dir's volume, made once (relax issue 163: on Windows the hard link failed with os
    /// error 17 and the run had no executable of its own). The two volumes: a file of ours in the
    /// temp folder and /dev/shm (a tmpfs of its own).
    #[test]
    fn a_loader_on_another_volume_is_linked_from_one_copy() {
        use std::os::unix::fs::MetadataExt;
        let own = format!("relax-link-across-{}", std::process::id());
        let from = std::env::temp_dir().join(&own);
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("loader"), b"the loader").unwrap();
        let (loader, dir) = (from.join("loader"), Path::new("/dev/shm").join(&own));
        std::fs::create_dir_all(&dir).unwrap();
        let devices = (std::fs::metadata(&loader).unwrap().dev(), std::fs::metadata(&dir).unwrap().dev());
        let direct = std::fs::hard_link(&loader, dir.join("direct")).map_err(|e| e.kind());
        let (a, b, copies) = (dir.join("a"), dir.join("b"), dir.join(COPIES));
        let linked = (link_loader(&loader, &a, &copies), link_loader(&loader, &b, &copies));
        let made: Vec<PathBuf> = std::fs::read_dir(&copies).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        let ones = made.len() == 1 && same_file(&a, &made[0]) && same_file(&b, &made[0]);
        let content = std::fs::read(&a).ok() == std::fs::read(&loader).ok();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&from);
        assert!(devices.0 != devices.1 && direct == Err(std::io::ErrorKind::CrossesDevices), "the loader and the folder are on two volumes: {devices:?} {direct:?}");
        assert!(linked.0.is_ok() && linked.1.is_ok(), "both links were made: {linked:?}");
        assert!(ones && content, "both are links of one copy of the loader: {made:?}");
    }
}
