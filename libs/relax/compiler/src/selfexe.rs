//! The program's own executable under the JIT: a hard link of the loader named as the program,
//! `<run dir>/exe/<loader key>/run/<name>` (`<name>.exe` on Windows), which the program's argv[0]
//! and std::env::current_exe() name. A spawn of that path runs this program again (std
//! process_unix.rs / process_windows.rs `self_exec_args`: RELAX_SELF_EXEC starts with it); a spawn
//! of the loader by any other name (RELAX_LOADER, an installed `makepad`) runs the loader's command
//! line as written, whatever its first word. A hard link, not a symlink or a copy: canonicalize
//! keeps its name, the OS runs it as the loader (the same file, so the same code signature on
//! macOS), and it takes no space beyond a directory entry while the loader file exists. A run dir
//! on another volume than the loader's links a copy of the loader there instead, one per loader
//! file (`link_loader`).
//!
//! The link is made once per loader file and program name, and every run of that program shares
//! it. The loader key names the folder (`loader_key`: the file's volume, number, length and last
//! write, so a rebuilt loader has another), and `same_file` says whether a link there is the
//! loader's file. No warm start makes a link: on macOS a new hard link of a file makes syspolicyd
//! scan the file again at its next exec, which held each exec of a 76 MB loader about 330 ms, and
//! now and then the process was killed in that wait. The folder of a loader file that was replaced
//! goes once no run of it is alive (`sweep`, when a link is made).
//!
//! A run is an entry in its link's folder, `+run/<pid>`: its command line (RELAX_SELF_EXEC's
//! words) and where its build environment is. A launch of the link that did not come through Relax
//! std's spawn (`sh -c "exec $EXE ..."`, a C library's exec: its arguments do not start with a
//! run's line) runs that run's program again, as std's spawn would have, built with that run's
//! build environment whatever environment the shell gave it (`relaunched`, `relaunch_build`,
//! relax issue 172). Its run is the one whose build environment's descriptor it holds, else its
//! nearest ancestor that is a run of the link, else the only live one; with none it says so and
//! ends. A test's package binary that a launcher clearing the environment (`env -i`) started
//! through its CARGO_BIN_EXE_<name> wrapper takes its test's run's build environment the same way
//! (`prepared_build`, relax issue 205). The environment is never in a file: it holds the person's
//! tokens, and a run that is killed leaves its entry. On Unix the run keeps it in a file that has
//! no name, by a descriptor an exec keeps, and the entry says which (`hand_build`); on Windows it
//! is `+run/<pid>.build`, which goes with the run's last handle.
//!
//! A test's links are laid out as cargo lays out target/<profile>: its executable in a `deps/`
//! folder and the package's binaries (their CARGO_BIN_EXE_<name> launchers) in the folder above it
//! (`add_bins`), so a test that finds a binary at `current_exe()/../../<name>`, as under cargo,
//! finds it (relax issue 139's second case). That folder is `t-<key>` (`test_key`: of what the
//! launchers' words hold), so a test run with other flags keeps its own binaries. On Windows a
//! binary there is `<name>.exe`, a link of the loader whose run's command line is in the folder's
//! `+bin` (`bin_start`).
//!
//! A process gets its program's name from the file it starts from (Linux's comm, macOS's p_comm:
//! what ps, top and a parent lookup show), as a cargo-built program has its own. So whoever starts
//! a run starts it from a link of the loader named as the program (`launch_link`, in the loader
//! key's `+launch` folder, shared as well): `makepad test` its targets' runs, a restart in place
//! the run again. A starter's `+launch/+run/<pid>` keeps the folder while the starter lives.
//!
//! A run removes its entry when it exits (os::proc::exit); a killed run's entry goes when the next
//! run of that program registers.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The folder of this run's links: its executable's (`run/`), or a test's above its `deps/`
/// (`t-<key>`), where the package's binaries go (`add_bins`)
static OWN_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The folder of this run's executable: OWN_DIR, a test's `deps/` in it
static LINK_DIR: OnceLock<PathBuf> = OnceLock::new();
/// The file name of this run's link
static NAME: OnceLock<String> = OnceLock::new();
/// The run a launch of its link from a shell is (`relaunched`): its pid and where its build
/// environment's descriptor is
static RELAUNCH: OnceLock<(u32, Option<(i32, u64, u64)>)> = OnceLock::new();
/// The links runs start from (`launch_link`), in a loader key's folder: a name no link has
const LAUNCH: &str = crate::dirs::LAUNCH_DIR;
/// The loader's copies the links are made of where the loader is on another volume
/// (`link_loader`), beside the loader keys' folders: a name no key has
const COPIES: &str = "+copy";
/// The runs of the links in a folder (`set_line`): a name no link has
const RUNS: &str = crate::dirs::RUN_ENTRIES;
/// The folder of a run's executable in its loader key's folder (a test's is `t-<key>`)
const RUN_DIR: &str = "run";

/// Writes the loader a link in `dir` was made of beside it (dirs::RUN_LOADER): a process that
/// runs from the link and was not told the loader (RELAX_LOADER: a program that relaunched itself
/// with a cleared environment) spawns that loader, never its own link. Written whole, and only
/// when it says another: other runs read it.
fn record_loader(dir: &Path, loader: &Path) {
    // a link made from another run's link (a program relaunched from it): that link's loader
    let real = if crate::dirs::is_run_link(loader) { crate::dirs::recorded_loader(loader).or_else(|| crate::dirs::loader_exe().ok()) } else { Some(loader.to_path_buf()) };
    let Some(real) = real else { return };
    let at = dir.join(crate::dirs::RUN_LOADER);
    let bytes = real.as_os_str().as_encoded_bytes();
    if std::fs::read(&at).ok().as_deref() != Some(bytes) {
        let part = dir.join(format!(".{}.loader", std::process::id()));
        if std::fs::write(&part, bytes).is_err() || std::fs::rename(&part, &at).is_err() {
            let _ = std::fs::remove_file(&part);
        }
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

/// The key of a test's links' folder (`t-<key>`): a hash of `words`, what its package binaries'
/// launchers hold apart from the loader (unitgraph.rs: the package, its flags and profile, the
/// native permission, the workspace), so a test run with other flags keeps its own binaries above
/// its `deps/`.
pub fn test_key(words: &[&str]) -> String {
    // FNV-1a, short (Windows' 260-character path limit)
    let mut h: u64 = 0xcbf29ce484222325;
    for b in words.iter().flat_map(|w| w.bytes().chain([0x1f])) {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (h ^ (h >> 32)) as u32)
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

/// This run's executable: the link of `loader`'s file named `name` (a package or binary name) in
/// the loader key's folder, made by the first run of that name and taken by every later one; the
/// reason when the run dir cannot hold it. `test`: a test's executable, in `t-<test>/deps/` as
/// cargo puts it (`test_key`). A `loader` that is a link of ours (an exec of the program's own
/// executable, a restart from its launch link) is the loader's file, so it finds the same link.
pub fn link(loader: &Path, name: &str, test: Option<&str>) -> Result<PathBuf, String> {
    let root = crate::dirs::relax_dir().join("exe");
    let of = file_id(loader).ok_or_else(|| format!("{}: the loader's file is not there", loader.display()))?;
    let key = key_of(&of);
    let dir = root.join(&key).join(test.map_or_else(|| RUN_DIR.to_string(), |t| format!("t-{t}")));
    let at = if test.is_some() { dir.join("deps") } else { dir.clone() };
    let link = at.join(file_name(name));
    let _ = NAME.set(file_name(name));
    let _ = OWN_DIR.set(dir.clone());
    let _ = LINK_DIR.set(at.clone());
    if !is_link_of(&link, &of, loader, &root.join(COPIES)) {
        make_link(&root, &key, loader, &[&dir, &at], &link)?;
    }
    link.canonicalize().map_err(|e| format!("{}: {e}", link.display()))
}

/// Makes `link`, a hard link of `loader`'s file, in the folder of its loader key `key` under `root`
/// (`<run dir>/exe`): a new program name, or a new loader file. The loader is recorded in that
/// folder and in each of `dirs` before the link is made, so no link is without it. The folders
/// of loader files no live run uses go first (`sweep`). Another run making the same link at once
/// is no failure.
fn make_link(root: &Path, key: &str, loader: &Path, dirs: &[&Path], link: &Path) -> Result<(), String> {
    sweep(root, key);
    let at = link.parent().unwrap_or(root);
    private_dirs(&at.join(RUNS)).map_err(|e| format!("{}: {e}", at.display()))?;
    record_loader(&root.join(key), loader);
    for d in dirs {
        record_loader(d, loader);
    }
    let copies = root.join(COPIES);
    match link_loader(loader, link, &copies) {
        Err(e) if !(e.kind() == std::io::ErrorKind::AlreadyExists && file_id(loader).is_some_and(|of| is_link_of(link, &of, loader, &copies))) => {
            Err(format!("hard link {} of {}: {e}", link.display(), loader.display()))
        }
        _ => Ok(()),
    }
}

/// Whether `link` is a link of the loader file `of` (at `loader`): that file, or where the link is
/// on another volume than the loader, the loader's copy there (`link_loader`).
fn is_link_of(link: &Path, of: &FileId, loader: &Path, copies: &Path) -> bool {
    match file_id(link) {
        Some(l) if l.volume == of.volume => l.index == of.index,
        Some(l) => copy_name(loader).and_then(|n| file_id(&copies.join(n))).is_some_and(|c| c.volume == l.volume && c.index == l.index),
        None => false,
    }
}

/// The name of the loader file's copy on another volume (`link_loader`): its length and
/// modification time, and its file name.
fn copy_name(loader: &Path) -> Option<String> {
    let m = std::fs::metadata(loader).ok()?;
    let modified = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    Some(format!("{:x}-{modified:x}-{}", m.len(), loader.file_name().unwrap_or_default().to_string_lossy()))
}

/// A hard link of `loader` at `to`. Where the file system cannot hold one because the loader is
/// on another volume (Windows' ERROR_NOT_SAME_DEVICE, Unix's EXDEV), a hard link of the loader's
/// copy in `copies`, a folder on `to`'s volume: a copy made once per loader file (`copy_name`),
/// which every later link shares. The copies of other loader files go (a link keeps the file it
/// was made of, and Windows keeps one in use). Before, a target dir on another drive than
/// relax.exe left the run without its own executable (relax issue 163: "hard link ... (os error
/// 17)").
fn link_loader(loader: &Path, to: &Path, copies: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(loader, to) {
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            let name = copy_name(loader).ok_or(e)?;
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

/// A link of `loader` named as the program `name`, in its loader key's `+launch` folder, for a
/// run this process starts to start from, so its process has the program's name from its start;
/// made once per loader file and name, as a run's own executable is (`link`), and kept while this
/// process lives (its `+launch/+run/<pid>`, which `remove_launch` takes). None where it cannot be
/// made (Windows, whose processes are not named by the file they start from): the caller starts
/// `loader`.
///
/// Every start a parent of ours makes goes through such a link, with no exec more (a cold start
/// has no floor): `makepad test` (its targets), a restart in place (`restart_exe`), devhub's node
/// (a JIT run of a program, devhub lease/exec.rs launch_link) and the app launcher (`app <id>`,
/// tools/run/launcher). A user's `makepad run` / `relax run` in a terminal has no parent of ours:
/// on macOS that run keeps the loader's name; on Linux it renames itself through /proc comm
/// (`name_process`).
pub fn launch_link(loader: &Path, name: &str) -> Option<PathBuf> {
    let path = launch_path(loader, name)?;
    let runs = path.parent()?.join(RUNS);
    let mark = runs.join(std::process::id().to_string());
    if !mark.exists() {
        drop_dead(&runs);
        std::fs::write(&mark, b"").ok()?;
    }
    Some(path)
}

/// The launch link of `loader`'s file named `name` (`launch_link`), made when it is not there.
#[cfg(unix)]
fn launch_path(loader: &Path, name: &str) -> Option<PathBuf> {
    let root = crate::dirs::relax_dir().join("exe");
    let of = file_id(loader)?;
    let key = key_of(&of);
    let dir = root.join(&key).join(LAUNCH);
    let path = dir.join(file_name(name));
    if !is_link_of(&path, &of, loader, &root.join(COPIES)) {
        make_link(&root, &key, loader, &[&dir], &path).ok()?;
    }
    Some(path)
}

#[cfg(not(unix))]
fn launch_path(_loader: &Path, _name: &str) -> Option<PathBuf> {
    None
}

/// What tells a file from another, and the file from what replaced it at its path: its volume and
/// its number there (Unix: device and inode; Windows: volume serial and file index), its length and
/// its last write.
struct FileId {
    volume: u64,
    index: u64,
    len: u64,
    written: u128,
}

#[cfg(unix)]
fn file_id(path: &Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    let written = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    Some(FileId { volume: m.dev(), index: m.ino(), len: m.len(), written })
}

#[cfg(windows)]
fn file_id(path: &Path) -> Option<FileId> {
    use makepad_windows_sys::Win32::Foundation::HANDLE;
    use makepad_windows_sys::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    // its attributes only: no read of the file, which a running image allows as any file does
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    let f = std::fs::OpenOptions::new().access_mode(FILE_READ_ATTRIBUTES).open(path).ok()?;
    let mut i = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(f.as_raw_handle()), &mut i) }.ok()?;
    let (w, high) = (i.ftLastWriteTime, |h: u32, l: u32| (h as u64) << 32 | l as u64);
    Some(FileId {
        volume: i.dwVolumeSerialNumber as u64,
        index: high(i.nFileIndexHigh, i.nFileIndexLow),
        len: high(i.nFileSizeHigh, i.nFileSizeLow),
        written: high(w.dwHighDateTime, w.dwLowDateTime) as u128,
    })
}

/// Whether `a` and `b` are one file: a hard link and the file it was made of are, a copy is not.
pub fn same_file(a: &Path, b: &Path) -> bool {
    match (file_id(a), file_id(b)) {
        (Some(a), Some(b)) => a.volume == b.volume && a.index == b.index,
        _ => false,
    }
}

/// The name of the folder of a loader file's links: its volume, number, length and last write,
/// so a loader that replaced it (a new build) has another. Only a name: whether a link in it is
/// that loader's file `same_file` says.
pub fn loader_key(loader: &Path) -> Option<String> {
    file_id(loader).map(|f| key_of(&f))
}

fn key_of(f: &FileId) -> String {
    format!("{:x}-{:x}-{:x}-{:x}", f.volume, f.index, f.len, f.written)
}

/// What a restart in place execs: `loader` by the run's launch link, so the process keeps its
/// program's name. (The process is a run: its entry keeps the folder, so it needs no starter's.)
pub fn restart_exe(loader: PathBuf) -> PathBuf {
    NAME.get().and_then(|n| launch_path(&loader, n)).unwrap_or(loader)
}

/// Removes this process's `+launch/+run/<pid>` (`makepad test` at its end): its launch links'
/// folder no longer stays for it.
pub fn remove_launch() {
    let Some(key) = crate::dirs::loader_exe().ok().and_then(|l| loader_key(&l)) else { return };
    let _ = std::fs::remove_file(crate::dirs::relax_dir().join("exe").join(key).join(LAUNCH).join(RUNS).join(std::process::id().to_string()));
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

/// Registers this run in its link's folder, `+run/<pid>`: its command line (RELAX_SELF_EXEC's
/// words, up to and with `--`) and where a launch of the link finds its build environment
/// (`hand_build`). The entries of runs that are gone go first. The folder is the owner's alone
/// (`private_dirs`), as the process's own environment is.
pub fn set_line(words: &[String]) -> Result<(), String> {
    let runs = LINK_DIR.get().ok_or("no link was made")?.join(RUNS);
    let entry = runs.join(std::process::id().to_string());
    drop_dead(&runs);
    let handed = hand_build(&runs, &entry).map_err(|e| format!("{}: {e}", runs.display()))?;
    let mut text = handed.into_bytes();
    text.push(b'\n');
    text.extend_from_slice(words.join("\u{1f}").as_bytes());
    // whole: a launch of the link may be reading the runs
    let part = runs.join(format!(".{}.entry", std::process::id()));
    std::fs::write(&part, &text).and_then(|_| std::fs::rename(&part, &entry)).map_err(|e| format!("{}: {e}", entry.display()))
}

/// The entries in `runs` of processes that are gone (killed: they did not remove their own).
fn drop_dead(runs: &Path) {
    for e in std::fs::read_dir(runs).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if pid != std::process::id() && !alive(pid) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// A run in a link's folder (`set_line`): its process, where its build environment's descriptor
/// is (Unix: descriptor, device, inode) and its command line.
struct Run {
    pid: u32,
    handed: Option<(i32, u64, u64)>,
    line: Vec<Vec<u8>>,
}

/// An entry's two parts: its first line, the descriptor words (`-` without them), and its command
/// line's words.
fn entry_parts(text: &[u8]) -> (Option<(i32, u64, u64)>, Vec<Vec<u8>>) {
    let (head, line) = match text.iter().position(|&b| b == b'\n') {
        Some(n) => (&text[..n], &text[n + 1..]),
        None => (text, &[][..]),
    };
    let handed = std::str::from_utf8(head).ok().and_then(|h| {
        let mut w = h.split(' ');
        let r = (w.next()?.parse::<i32>().ok().filter(|&fd| fd >= 0)?, w.next()?.parse().ok()?, w.next()?.parse().ok()?);
        w.next().is_none().then_some(r)
    });
    (handed, line.split(|&b| b == 0x1f).map(<[u8]>::to_vec).collect())
}

/// The runs of the links in `dir` (its `+run`); None where it has none (no run's executable is
/// there).
fn runs_in(dir: &Path) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    for e in std::fs::read_dir(dir.join(RUNS)).ok()?.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(text) = std::fs::read(e.path()) else { continue };
        let (handed, line) = entry_parts(&text);
        if line.len() > 1 {
            runs.push(Run { pid, handed, line });
        }
    }
    Some(runs)
}

/// The run's build environment as a launch of its link takes it (`hand_build`).
static HANDED: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);

/// What this module holds of a run, as a fresh process has it (`run_end`): the ended run's build
/// environment is no longer handed to a launch of its link (its file goes with the descriptor; a
/// child that inherited one keeps its own). The link and its folder are the process's
/// (process_statics.tsv).
pub(crate) unsafe fn reset_statics() {
    drop(HANDED.lock().unwrap_or_else(|e| e.into_inner()).take());
}

#[cfg(unix)]
extern "C" {
    fn dup(fd: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn getppid() -> i32;
}

/// The build environment (progenv `build_wire`, the person's whole environment as the run started,
/// tokens included) goes to a launch of the link in a file without a name: made in the owner-only
/// folder and unlinked before anything is written to it, held open by a descriptor without
/// close-on-exec, which a shell's or a C library's exec of the link keeps. The run's entry
/// (`entry`) names that descriptor by the words returned: its number and the file's device and
/// inode, never a variable. When the run ends, killed or not, the file goes with its last
/// descriptor, so no run leaves the environment on disk. A run that starts again in place (an exec
/// of this process) closes the one its image before held.
#[cfg(unix)]
fn hand_build(runs: &Path, entry: &Path) -> std::io::Result<String> {
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut held = HANDED.lock().unwrap_or_else(|e| e.into_inner());
    if held.take().is_none() {
        if let (Some((fd, dev, ino)), _) = entry_parts(&std::fs::read(entry).unwrap_or_default()) {
            drop(open_handed(fd, dev, ino));
        }
    }
    let part = runs.join(format!(".{}.build", std::process::id()));
    let _ = std::fs::remove_file(&part);
    let made = std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).open(&part)?;
    std::fs::remove_file(&part)?;
    (&made).write_all(&crate::progenv::build_wire())?;
    // (dup's descriptor has no close-on-exec: std opens every file with it)
    let fd = unsafe { dup(made.as_raw_fd()) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let kept = unsafe { std::fs::File::from_raw_fd(fd) };
    let m = kept.metadata()?;
    *held = Some(kept);
    Ok(format!("{fd} {} {}", m.dev(), m.ino()))
}

/// On Windows (no exec) the build environment is `+run/<pid>.build`, made to be deleted when its
/// last handle closes: the run holds it open, so it goes when the run ends, killed or not.
#[cfg(windows)]
fn hand_build(runs: &Path, _entry: &Path) -> std::io::Result<String> {
    use std::io::Write;
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
    let mut held = HANDED.lock().unwrap_or_else(|e| e.into_inner());
    // (the run before in this process: its file goes with its handle)
    drop(held.take());
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).custom_flags(FILE_FLAG_DELETE_ON_CLOSE).open(runs.join(format!("{}.build", std::process::id())))?;
    f.write_all(&crate::progenv::build_wire())?;
    *held = Some(f);
    Ok("-".to_string())
}

/// Descriptor `fd` of this process when it is open on the file `dev`/`ino`, to own (and close).
#[cfg(unix)]
fn open_handed(fd: i32, dev: u64, ino: u64) -> Option<std::fs::File> {
    use std::os::fd::FromRawFd;
    use std::os::unix::fs::MetadataExt;
    const F_GETFD: i32 = 1;
    if unsafe { fcntl(fd, F_GETFD) } < 0 {
        return None;
    }
    // not closed unless it is the file named: another descriptor of that number is someone else's
    let f = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
    f.metadata().is_ok_and(|m| m.dev() == dev && m.ino() == ino).then(|| std::mem::ManuallyDrop::into_inner(f))
}

/// Whether this process holds descriptor `fd` open on the file `dev`/`ino` (left open).
#[cfg(unix)]
fn holds(fd: i32, dev: u64, ino: u64) -> bool {
    use std::os::fd::IntoRawFd;
    open_handed(fd, dev, ino).map(IntoRawFd::into_raw_fd).is_some()
}

/// The build environment of the run this process was launched for (`relaunched`), which it
/// handed (`hand_build`): what the loader builds with before anything reads the environment
/// (progenv `init_handed`). Its descriptor, which the exec kept, is read and closed; on Linux,
/// where a launcher closed it on the way, that run's own is read through /proc. None, said in one
/// line, when neither is there.
#[cfg(unix)]
pub fn relaunch_build() -> Option<Vec<u8>> {
    let &(pid, handed) = RELAUNCH.get()?;
    let build = handed.and_then(|(fd, dev, ino)| read_handed(&[(pid, fd, dev, ino)], pid));
    if build.is_none() {
        eprintln!("relax: the build environment of the run whose executable this is did not reach this process (its descriptor was closed on the way); it builds with its own");
    }
    build
}

/// The first of the handed build environments `at` (runs' entries: pid, descriptor, device, inode) whose descriptor this
/// process holds, read and closed; on Linux, where a launcher closed it on the way, `parent`'s own
/// through /proc.
#[cfg(unix)]
fn read_handed(at: &[(u32, i32, u64, u64)], parent: u32) -> Option<Vec<u8>> {
    use std::os::unix::fs::FileExt;
    let read = |f: &std::fs::File| -> Option<Vec<u8>> {
        let mut b = vec![0; f.metadata().ok()?.len() as usize];
        f.read_exact_at(&mut b, 0).ok()?;
        Some(b)
    };
    if let Some(f) = at.iter().find_map(|&(_, fd, dev, ino)| open_handed(fd, dev, ino)) {
        return read(&f);
    }
    #[cfg(target_os = "linux")]
    for &(pid, fd, dev, ino) in at.iter().filter(|h| h.0 == parent) {
        if let Ok(f) = std::fs::File::open(format!("/proc/{pid}/fd/{fd}")) {
            use std::os::unix::fs::MetadataExt;
            if f.metadata().is_ok_and(|m| m.dev() == dev && m.ino() == ino) {
                return read(&f);
            }
        }
    }
    let _ = parent;
    None
}

/// A test's package binary started through its CARGO_BIN_EXE_<name> wrapper (unitgraph.rs: it sets
/// RELAX_ARG0, its words name the test's run dir) without the build environment Relax std's spawn
/// hands it (BUILD_ENV): a launcher between the test and the binary cleared the environment
/// (`env -i`). The descriptor the test's run hands its build environment by (`hand_build`) crossed
/// those execs, as it does for a launch of a run's own link: the run in `<run dir>/exe` whose
/// entry names a descriptor this process holds started it, the parent's first (relax issue 205:
/// without its CARGO_HOME the binary resolved its registry packages elsewhere, its test's native
/// build no longer matched, and it built again). None, said in one line when the binary was
/// started that way, when no run handed one.
#[cfg(unix)]
pub fn prepared_build(args: &[Vec<u8>]) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    if std::env::var_os("RELAX_ARG0").is_none() || std::env::var_os("RELAX_BUILD_ENV").is_some() {
        return None;
    }
    let run_dir = args.windows(2).find(|w| w[0] == b"--run-dir").map(|w| Path::new(std::ffi::OsStr::from_bytes(&w[1])).to_path_buf())?;
    let parent = unsafe { getppid() } as u32;
    // every run's entry: <loader key>/<run or t-<key>>[/deps]/+run/<pid>
    let mut at: Vec<(u32, i32, u64, u64)> = Vec::new();
    for key in std::fs::read_dir(run_dir.join("exe")).into_iter().flatten().flatten() {
        for sub in std::fs::read_dir(key.path()).into_iter().flatten().flatten() {
            for dir in [sub.path(), sub.path().join("deps")] {
                at.extend(runs_in(&dir).unwrap_or_default().into_iter().filter_map(|r| r.handed.map(|(fd, dev, ino)| (r.pid, fd, dev, ino))));
            }
        }
    }
    at.sort_by_key(|h| h.0 != parent);
    let build = read_handed(&at, parent);
    if build.is_none() {
        eprintln!("relax: the build environment of the test run that prepared this binary did not reach it (a launcher cleared the environment and closed its descriptor); it builds with its own");
    }
    build
}

/// On Windows a test's binary is started by its `.cmd` or a link above `deps/` (`bin_start`), and
/// a launcher that clears the environment drops the build environment Relax std's spawn handed it.
#[cfg(windows)]
pub fn prepared_build(_args: &[Vec<u8>]) -> Option<Vec<u8>> {
    None
}

#[cfg(windows)]
pub fn relaunch_build() -> Option<Vec<u8>> {
    let &(pid, _) = RELAUNCH.get()?;
    std::fs::read(std::env::current_exe().ok()?.parent()?.join(RUNS).join(format!("{pid}.build"))).ok()
}

/// The folder's command lines of the package's binaries on Windows (`add_bins`): a name no link has.
const BIN: &str = crate::dirs::BIN_LINES;

/// The package's binaries in this test's folder, as cargo has them in target/<profile> beside a
/// test's `deps/`: `launchers` are the CARGO_BIN_EXE_<name> values, each linked (else copied)
/// under its own file name, again when the launcher is a new file (another run's test wrote it
/// with other words). A test that finds a binary by that layout ran nothing before. On Windows the
/// launcher is a `.cmd`, and no `.exe` by the binary's name was there, which is where cargo has it
/// (relax PR 153: NotFound): there the binary is `<name>.exe`, a link of the loader, and its run's
/// words (unitgraph.rs, beside the `.cmd`) go in the folder's `+bin` under that name, which the
/// loader started by it takes (`bin_start`). Returns where each one is.
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
            let (lines, to) = (dir.join(BIN), dir.join(&file));
            let words_there = std::fs::read(lines.join(&file)).ok().as_deref() == Some(&words[..]);
            if words_there || (std::fs::create_dir_all(&lines).is_ok() && std::fs::write(lines.join(&file), &words).is_ok()) {
                let copies = dir.parent().and_then(Path::parent).map(|r| r.join(COPIES)).unwrap_or_default();
                if !file_id(&loader).is_some_and(|of| is_link_of(&to, &of, &loader, &copies)) {
                    let _ = link_loader(&loader, &to, &copies);
                }
                placed.push(to);
            }
            continue;
        }
        let Some(file) = from.file_name() else { continue };
        let to = dir.join(file);
        if !same_file(from, &to) {
            // whole: another run of this test may be starting it
            let part = dir.join(format!(".{}.{}", std::process::id(), file.to_string_lossy()));
            let _ = std::fs::remove_file(&part);
            if std::fs::hard_link(from, &part).is_err() {
                let _ = std::fs::copy(from, &part);
            }
            if std::fs::rename(&part, &to).is_err() {
                let _ = std::fs::remove_file(&part);
            }
        }
        placed.push(to);
    }
    placed
}

/// Whether `exe` is a run's own executable (its runs' folder beside it), a link a run was started
/// from (`launch_link`), or a package binary above a test's `deps/` (its line in `+bin`): none is
/// the loader's own path (a launch of a run's link runs that run's program, as one of a package
/// binary runs that binary), so the loader running from one spawns itself by RELAX_LOADER
/// (dirs::loader_exe).
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

/// A launch of a run's link whose arguments do not start with a run's command line (Relax std's
/// spawn and exec put it after argv[0], an exec without the `--silent` at its index 2): `args` with
/// that run's line after argv[0], as std's spawn gives them (`which_run`); a launch with no run to
/// be says so in one line and ends (exit 2). None for any other launch of the loader, and for a
/// start from a launch link, whose command line is the loader's as written. (The arguments, not
/// RELAX_SELF_EXEC, tell: a C library's exec passes this process's own environment, which names
/// it, with the program's arguments alone.)
pub fn relaunched(args: &[Vec<u8>]) -> Option<Vec<Vec<u8>>> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    if dir.file_name().is_some_and(|n| n == LAUNCH) {
        return None;
    }
    let runs = runs_in(dir)?;
    let rest = args.get(1..).unwrap_or_default();
    if runs.iter().any(|r| starts_with_line(rest, &r.line)) {
        return None;
    }
    let run = match which_run(&runs) {
        Ok(r) => r,
        Err(why) => {
            eprintln!("relax: {}: {why}", exe.display());
            std::process::exit(2);
        }
    };
    let _ = RELAUNCH.set((run.pid, run.handed));
    let mut out = run.line.clone();
    out[0] = args.first()?.clone();
    out.extend(rest.iter().cloned());
    Some(out)
}

/// Whether `rest` (the arguments after argv[0]) start with a run's `line` after its first word, or
/// with that line without the `--silent` a run adds at its index 2 (which an exec drops).
fn starts_with_line(rest: &[Vec<u8>], line: &[Vec<u8>]) -> bool {
    let mut unsilenced = line[1..].to_vec();
    if unsilenced.get(1).is_some_and(|w| w == b"--silent") {
        unsilenced.remove(1);
    }
    rest.starts_with(&line[1..]) || rest.starts_with(&unsilenced)
}

/// Which of the link's `runs` a launch of it that came through no spawn of Relax std is: the one
/// whose build environment's descriptor this process holds (an exec keeps it; of several, the
/// nearest in its lineage: this process itself, a run that execs its executable through a shell,
/// then its ancestors), else the nearest in its lineage that is a run of the link (a launcher
/// closed the descriptor; Windows has none), else the only live run; else why none is.
fn which_run(runs: &[Run]) -> Result<&Run, String> {
    fn nearest<'a>(lineage: &[u32], of: &[&'a Run]) -> Option<&'a Run> {
        lineage.iter().find_map(|&a| of.iter().find(|r| r.pid == a).copied())
    }
    let lineage = lineage();
    #[cfg(unix)]
    {
        let held: Vec<&Run> = runs.iter().filter(|r| r.handed.is_some_and(|(fd, dev, ino)| holds(fd, dev, ino))).collect();
        if let Some(r) = nearest(&lineage, &held).or(held.first().copied()) {
            return Ok(r);
        }
    }
    if let Some(r) = nearest(&lineage, &runs.iter().collect::<Vec<_>>()) {
        return Ok(r);
    }
    let live: Vec<&Run> = runs.iter().filter(|r| alive(r.pid)).collect();
    match live[..] {
        [r] => Ok(r),
        [] => Err("no run of this program is live, and a launch of a run's own executable runs that run's program again".to_string()),
        _ => Err(format!("{} runs of this program are live and none of them started this process, so it cannot tell whose program to run again", live.len())),
    }
}

/// This process, then its ancestors, its parent first, up to the first that is gone (at most 64).
fn lineage() -> Vec<u32> {
    let mut out = vec![std::process::id()];
    let mut p = parent_of(std::process::id());
    while let Some(pid) = p.filter(|&p| p > 1 && out.len() <= 64 && !out.contains(&p)) {
        out.push(pid);
        p = parent_of(pid);
    }
    out
}

/// Removes this run's entry (os::proc::exit).
pub fn remove() {
    if let Some(d) = LINK_DIR.get() {
        let _ = std::fs::remove_file(d.join(RUNS).join(std::process::id().to_string()));
    }
}

/// The folders under `root` (`<run dir>/exe`) that no live process uses, but the loader key
/// `keep`'s: a loader key's whose `+loader` names a file that has another key now (a new build)
/// and none of whose runs and starters is alive, and a run's own folder from before runs shared
/// their links (named by its pid) whose process is gone. On Windows a folder that holds an image
/// in use stays, and a later sweep takes it.
fn sweep(root: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == keep || name == COPIES {
            continue;
        }
        let used = match name.parse::<u32>() {
            Ok(pid) => alive(pid),
            Err(_) => crate::dirs::recorded_loader(&e.path().join(crate::dirs::RUN_LOADER)).and_then(|l| loader_key(&l)).is_some_and(|k| k == name) || live_under(&e.path()),
        };
        if !used {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Whether a run or a starter of the loader key folder `dir` is alive: an entry in a `+run` of any
/// of its folders (`run/`, `+launch/`, a test's `t-<key>/deps/`).
fn live_under(dir: &Path) -> bool {
    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|sub| {
        [sub.path().join(RUNS), sub.path().join("deps").join(RUNS)]
            .iter()
            .any(|runs| std::fs::read_dir(runs).into_iter().flatten().flatten().any(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()).is_some_and(alive)))
    })
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

/// The parent of the process `pid`, None when it is gone or not ours to see.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn parent_of(pid: u32) -> Option<u32> {
    // `pid (name) state ppid ...`: the name may hold spaces and parentheses, so after its last `)`
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 1..].split_whitespace().nth(1)?.parse().ok()
}

#[cfg(target_vendor = "apple")]
pub fn parent_of(pid: u32) -> Option<u32> {
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut u8, size: i32) -> i32;
    }
    // struct proc_bsdshortinfo (64 bytes): pbsi_pid, then pbsi_ppid
    const PROC_PIDT_SHORTBSDINFO: i32 = 13;
    let mut buf = [0u8; 64];
    let n = unsafe { proc_pidinfo(i32::try_from(pid).ok()?, PROC_PIDT_SHORTBSDINFO, 0, buf.as_mut_ptr(), buf.len() as i32) };
    let word = |at: usize| u32::from_ne_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]]);
    (n == buf.len() as i32 && word(0) == pid).then(|| word(4))
}

#[cfg(windows)]
pub fn parent_of(pid: u32) -> Option<u32> {
    use makepad_windows_sys::Win32::Foundation::CloseHandle;
    use makepad_windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W::default();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut parent = None;
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more && parent.is_none() {
            if e.th32ProcessID == pid {
                parent = Some(e.th32ParentProcessID);
            }
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        parent
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
pub fn parent_of(_pid: u32) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the links of a loader are told by: a hard link and its file are one file, with one
    /// key; a copy is another file; the file a new build put at the loader's path has another key
    /// and is no longer the link's; a child's parent is this process.
    #[test]
    fn a_link_is_its_file_and_a_child_names_its_parent() {
        let dir = std::env::temp_dir().join(format!("relax-same-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (file, link, copy, new) = (dir.join("loader"), dir.join("link"), dir.join("copy"), dir.join("new"));
        std::fs::write(&file, b"the loader").unwrap();
        std::fs::hard_link(&file, &link).unwrap();
        std::fs::copy(&file, &copy).unwrap();
        let key = loader_key(&file);
        let linked = (same_file(&file, &link), loader_key(&link) == key, same_file(&file, &copy));
        // as cargo replaces a binary: a new file renamed over it
        std::fs::write(&new, b"the loader").unwrap();
        std::fs::rename(&new, &file).unwrap();
        let replaced = (loader_key(&file) != key, same_file(&file, &link));
        // a child that waits for the end of its input
        let waits = if cfg!(windows) { Path::new(&std::env::var_os("SystemRoot").unwrap_or_default()).join("System32").join("sort.exe") } else { PathBuf::from("cat") };
        let mut child = std::process::Command::new(&waits).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
        let parent = parent_of(child.id());
        drop(child.stdin.take());
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(key.is_some(), "the loader has a key");
        assert_eq!(linked, (true, true, false), "a hard link is its file and has its key; a copy is another file");
        assert_eq!(replaced, (true, false), "the file that replaced the loader has another key and is not the link's");
        assert_eq!(parent, Some(std::process::id()), "the parent of a child of this process");
    }

    // (Linux: /dev/shm is a volume of its own a test can write to; the Mac and the Windows node
    // have none: macOS shows its system and data volumes as one device, and the Windows node has
    // only C:)
    /// A loader on another volume than the run dir: each link is a hard link of one copy of it on
    /// the run dir's volume, made once (relax issue 163: on Windows the hard link failed with os
    /// error 17 and the run had no executable of its own). The two volumes: a file of ours in the
    /// temp folder and /dev/shm (a tmpfs of its own).
    #[cfg(target_os = "linux")]
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
