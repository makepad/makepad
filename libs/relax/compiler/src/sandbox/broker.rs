//! The files of a confined build script (script.rs): the folders it may reach (its roots), its
//! own fd table, and the walk that resolves each path it names.
//!
//! - Roots: its package's source (read) and OUT_DIR (read and write), each held as a folder fd
//!   the broker opened from the path its parent gave, matched by that spelling and by its
//!   canonical one. Folders inside them it never reaches (the run's target folder, Relax's
//!   cache) are rows of identity, (st_dev, st_ino): a walk never enters one, however it is
//!   spelled. A root inside an excluded folder (OUT_DIR inside target) is its own row and wins.
//! - The walk: a path is resolved from a root's fd one component at a time, with `fstatat`,
//!   `openat(.., O_NOFOLLOW)` and `readlinkat` on the folder fd it walked; no call ever takes a
//!   path the script gave. A symlink's target is walked again under the same rules, up to 40
//!   links (ELOOP after). `..` goes up through the folders it walked; above a root it goes on
//!   from the root's canonical parent. Where the path leaves every root it is resolved by its
//!   spelling alone, and the walk enters a root again only where the spelling names one: what is
//!   outside the roots is never touched. A read outside is ENOENT, a write EACCES; an excluded
//!   folder is ENOENT.
//! - The fd table: the script's fds are indexes of the broker's own table, never host fds. 0 is
//!   the null device, 1 and 2 the child's stdout and stderr.
//! - The current folder is the broker's (the manifest folder, as cargo starts a script): `chdir`
//!   moves it within the roots, and the process's own never changes.
//! - A git package's source is a store folder of Relax's VFS (a virtual row): its committed files
//!   only (relax-base vfs.rs `store_*`), never a folder mount's disk, the edits folder or a link's
//!   target, and any other path of the VFS is "not virtual", so the walk answers it. A script whose
//!   manifest folder is virtual has std's virtual current folder (RELAX_WORKSPACE) and no disk one.

use std::ffi::{c_char, c_int, CString};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use super::script::Roots;

pub(crate) mod c {
    #![allow(dead_code)]
    use std::ffi::c_int;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod os {
        use std::ffi::c_int;
        pub const O_CREAT: c_int = 0x40;
        pub const O_EXCL: c_int = 0x80;
        pub const O_TRUNC: c_int = 0x200;
        pub const O_APPEND: c_int = 0x400;
        pub const O_NONBLOCK: c_int = 0x800;
        #[cfg(target_arch = "x86_64")]
        pub const O_DIRECTORY: c_int = 0x1_0000;
        #[cfg(target_arch = "x86_64")]
        pub const O_NOFOLLOW: c_int = 0x2_0000;
        #[cfg(target_arch = "x86_64")]
        pub const O_LARGEFILE: c_int = 0x8000;
        #[cfg(not(target_arch = "x86_64"))]
        pub const O_DIRECTORY: c_int = 0x4000;
        #[cfg(not(target_arch = "x86_64"))]
        pub const O_NOFOLLOW: c_int = 0x8000;
        #[cfg(not(target_arch = "x86_64"))]
        pub const O_LARGEFILE: c_int = 0x2_0000;
        pub const O_CLOEXEC: c_int = 0x8_0000;
        pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
        pub const AT_REMOVEDIR: c_int = 0x200;
        pub const F_DUPFD_CLOEXEC: c_int = 1030;
        pub const ELOOP: i32 = 40;
        pub const ENAMETOOLONG: i32 = 36;
        pub const ENOSYS: i32 = 38;
        /// struct stat (x86_64: 144 bytes, mode at 24; asm-generic: 128, mode at 16)
        #[cfg(target_arch = "x86_64")]
        pub const STAT_SIZE: usize = 144;
        #[cfg(not(target_arch = "x86_64"))]
        pub const STAT_SIZE: usize = 128;
    }
    #[cfg(target_vendor = "apple")]
    mod os {
        use std::ffi::c_int;
        pub const O_NONBLOCK: c_int = 0x4;
        pub const O_APPEND: c_int = 0x8;
        pub const O_NOFOLLOW: c_int = 0x100;
        pub const O_CREAT: c_int = 0x200;
        pub const O_TRUNC: c_int = 0x400;
        pub const O_EXCL: c_int = 0x800;
        pub const O_DIRECTORY: c_int = 0x10_0000;
        pub const O_CLOEXEC: c_int = 0x100_0000;
        pub const O_LARGEFILE: c_int = 0;
        pub const AT_SYMLINK_NOFOLLOW: c_int = 0x20;
        pub const AT_REMOVEDIR: c_int = 0x80;
        pub const F_DUPFD_CLOEXEC: c_int = 67;
        pub const ELOOP: i32 = 62;
        pub const ENAMETOOLONG: i32 = 63;
        pub const ENOSYS: i32 = 78;
        /// struct stat (64-bit inodes): 144 bytes, mode a u16 at 4
        pub const STAT_SIZE: usize = 144;
    }
    pub use os::*;
    pub const O_RDONLY: c_int = 0;
    pub const O_WRONLY: c_int = 1;
    pub const O_RDWR: c_int = 2;
    pub const O_ACCMODE: c_int = 3;
    pub const F_DUPFD: c_int = 0;
    pub const F_GETFD: c_int = 1;
    pub const F_SETFD: c_int = 2;
    pub const F_GETFL: c_int = 3;
    pub const F_SETFL: c_int = 4;
    pub const FD_CLOEXEC: c_int = 1;
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const EBADF: i32 = 9;
    pub const EACCES: i32 = 13;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENOTTY: i32 = 25;
    pub const ESPIPE: i32 = 29;
    pub const ERANGE: i32 = 34;
    /// the links one resolution follows (Linux's MAXSYMLINKS)
    pub const MAX_LINKS: u32 = 40;
}

#[cfg(target_vendor = "apple")]
type ModeT = u16;
#[cfg(not(target_vendor = "apple"))]
type ModeT = u32;

extern "C" {
    fn open(path: *const c_char, flags: c_int, ...) -> c_int;
    fn openat(dir: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "fstatat$INODE64")]
    fn fstatat(dir: c_int, path: *const c_char, buf: *mut u8, flags: c_int) -> c_int;
    #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "fstat$INODE64")]
    fn fstat(fd: c_int, buf: *mut u8) -> c_int;
    fn readlinkat(dir: c_int, path: *const c_char, buf: *mut u8, len: usize) -> isize;
    fn mkdirat(dir: c_int, path: *const c_char, mode: ModeT) -> c_int;
    fn unlinkat(dir: c_int, path: *const c_char, flags: c_int) -> c_int;
    fn renameat(from_dir: c_int, from: *const c_char, to_dir: c_int, to: *const c_char) -> c_int;
    fn fchmod(fd: c_int, mode: ModeT) -> c_int;
    #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "fdopendir$INODE64")]
    fn fdopendir(fd: c_int) -> *mut std::ffi::c_void;
}

/// The host's errno of the call that just failed.
pub(crate) fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(c::EINVAL)
}

/// A `struct stat` as the platform's libc writes it.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
pub(crate) struct Stat(pub [u8; 160]);

impl Stat {
    fn zeroed() -> Stat {
        Stat([0; 160])
    }
    fn u64_at(&self, o: usize) -> u64 {
        u64::from_ne_bytes(self.0[o..o + 8].try_into().unwrap())
    }
    pub(crate) fn mode(&self) -> u32 {
        #[cfg(target_vendor = "apple")]
        return u16::from_ne_bytes([self.0[4], self.0[5]]) as u32;
        #[cfg(all(not(target_vendor = "apple"), target_arch = "x86_64"))]
        return u32::from_ne_bytes(self.0[24..28].try_into().unwrap());
        #[cfg(all(not(target_vendor = "apple"), not(target_arch = "x86_64")))]
        return u32::from_ne_bytes(self.0[16..20].try_into().unwrap());
    }
    fn id(&self) -> (u64, u64) {
        #[cfg(target_vendor = "apple")]
        let dev = u32::from_ne_bytes(self.0[0..4].try_into().unwrap()) as u64;
        #[cfg(not(target_vendor = "apple"))]
        let dev = self.u64_at(0);
        (dev, self.u64_at(8))
    }
    pub(crate) fn is_dir(&self) -> bool {
        self.mode() & 0o170000 == 0o040000
    }
    fn is_link(&self) -> bool {
        self.mode() & 0o170000 == 0o120000
    }
    /// The bytes a script's `struct stat` gets.
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.0[..c::STAT_SIZE]
    }
}

fn fstat_fd(fd: RawFd) -> Result<Stat, i32> {
    let mut st = Stat::zeroed();
    if unsafe { fstat(fd, st.0.as_mut_ptr()) } != 0 {
        return Err(last_errno());
    }
    Ok(st)
}

/// The path components of `p`, without empty ones and `.`.
fn components(p: &[u8]) -> Vec<Vec<u8>> {
    p.split(|b| *b == b'/').filter(|c| !c.is_empty() && *c != b".").map(|c| c.to_vec()).collect()
}

fn join(comps: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    for c in comps {
        out.push(b'/');
        out.extend_from_slice(c);
    }
    if out.is_empty() {
        out.push(b'/');
    }
    out
}

struct Root {
    fd: OwnedFd,
    write: bool,
    /// its spellings as components: the given one and the canonical one
    names: Vec<Vec<Vec<u8>>>,
    canon: Vec<Vec<u8>>,
}

/// One of the script's fds.
pub(crate) enum Fd {
    /// the child's own stdout (1) or stderr (2)
    Std(RawFd),
    /// the null device: reads end, writes vanish
    Null,
    /// a file or folder the walk opened, or an anonymous file; `write`: under a write root
    File { fd: OwnedFd, write: bool },
    /// an end of a pipe the script made
    Pipe(OwnedFd),
}

/// What `open` gives.
pub(crate) enum Opened {
    Null,
    File { fd: OwnedFd, write: bool },
}

/// The folder a walk ended in.
enum DirFd {
    Root(RawFd),
    Own(OwnedFd),
}

impl DirFd {
    fn raw(&self) -> RawFd {
        match self {
            DirFd::Root(fd) => *fd,
            DirFd::Own(fd) => fd.as_raw_fd(),
        }
    }
}

/// Where a walk ended: the folder holding `name` (or the folder itself when `name` is None),
/// under root `root` at components `below`; `last`: the entry's lstat (the folder's own stat
/// when `name` is None), None when it does not exist.
struct Found {
    dir: DirFd,
    name: Option<CString>,
    root: usize,
    below: Vec<Vec<u8>>,
    last: Option<Stat>,
}

impl Found {
    /// The name an `*at` call takes: the entry, or the folder itself.
    fn at_name(&self) -> &std::ffi::CStr {
        self.name.as_deref().unwrap_or(c".")
    }
}

/// Where a walk is: outside every root (by spelling), or in a root under folders it opened.
enum At {
    Out(Vec<Vec<u8>>),
    In { root: usize, below: Vec<Vec<u8>>, stack: Vec<OwnedFd> },
}

pub(crate) struct Broker {
    roots: Vec<Root>,
    excluded: Vec<(u64, u64)>,
    /// the store folders it reads (virtual paths, normalized)
    stores: Vec<String>,
    /// the current folder on disk: a root and the components under it (None: a virtual one)
    cwd: Option<(usize, Vec<Vec<u8>>)>,
    /// the virtual current folder (a git package's manifest folder)
    pub(crate) virtual_cwd: Option<String>,
    fds: Vec<Option<Fd>>,
    /// open folder listings: the host's `DIR*` and the script's dirent block (script.rs)
    dirs: Vec<Option<(usize, u64)>>,
    /// the null device as the host's fd (a Null fd's stat, and handed to a process: script.rs)
    pub(crate) null: OwnedFd,
    /// what the script asked for and was refused, once each (`withhold`): "read <path>",
    /// "write <path>", "run <program>"
    withheld: std::cell::RefCell<Vec<String>>,
}

fn host_open(path: &[u8], flags: c_int) -> Result<OwnedFd, String> {
    let c = CString::new(path).map_err(|_| "a NUL in the path".to_string())?;
    let fd = unsafe { open(c.as_ptr(), flags | c::O_CLOEXEC, 0 as std::ffi::c_uint) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

impl Broker {
    /// The roots of `spec`, opened (a folder that cannot be opened: the run is refused).
    pub(crate) fn new(spec: &Roots) -> Result<Broker, String> {
        let say = |p: &[u8], e: String| format!("the confined build script's folder {} cannot be opened: {e}", String::from_utf8_lossy(p));
        let mut roots = Vec::new();
        for (paths, write) in [(&spec.read, false), (&spec.write, true)] {
            for p in paths {
                if !p.starts_with(b"/") {
                    return Err(say(p, "not an absolute path".into()));
                }
                let fd = host_open(p, c::O_RDONLY | c::O_DIRECTORY).map_err(|e| say(p, e))?;
                let canon = std::fs::canonicalize(std::ffi::OsStr::from_bytes(p)).map_err(|e| say(p, e.to_string()))?;
                let canon = components(canon.as_os_str().as_encoded_bytes());
                // (the given spelling resolved by text: the parent gives absolute, normal paths)
                let mut given: Vec<Vec<u8>> = Vec::new();
                for c in components(p) {
                    if c == b".." {
                        given.pop();
                    } else {
                        given.push(c);
                    }
                }
                let mut names = vec![canon.clone()];
                if given != canon {
                    names.push(given);
                }
                // another spelling of the folder (a folder mount's virtual path)
                for (spelling, folder) in &spec.alias {
                    if folder == p && !names.contains(&components(spelling)) {
                        names.push(components(spelling));
                    }
                }
                roots.push(Root { fd, write, names, canon });
            }
        }
        let mut excluded = Vec::new();
        for p in &spec.excluded {
            // (a folder that is not there yet has nothing to keep out)
            if let Ok(fd) = host_open(p, c::O_RDONLY | c::O_DIRECTORY) {
                excluded.push(fstat_fd(fd.as_raw_fd()).map_err(|e| say(p, std::io::Error::from_raw_os_error(e).to_string()))?.id());
            }
        }
        let null = host_open(b"/dev/null", c::O_RDWR).map_err(|e| say(b"/dev/null", e))?;
        let stores = spec.store.iter().map(|p| crate::vfs::normalize_path(&String::from_utf8_lossy(p))).collect();
        let mut b = Broker { roots, excluded, stores, cwd: None, virtual_cwd: None, fds: vec![Some(Fd::Null), Some(Fd::Std(1)), Some(Fd::Std(2))], dirs: Vec::new(), null, withheld: Default::default() };
        if b.roots.is_empty() {
            return Err("the confined build script has no folders".into());
        }
        if let Some(v) = b.store_path(&spec.cwd) {
            if !matches!(crate::vfs::store_kind(&v), Some(crate::vfs::StoreKind::Dir)) {
                return Err(say(&spec.cwd, "not a folder of its store".into()));
            }
            b.virtual_cwd = Some(v);
        } else {
            let f = b.walk(&spec.cwd, true, false).map_err(|e| say(&spec.cwd, std::io::Error::from_raw_os_error(e).to_string()))?;
            b.cwd = Some(b.folder_of(f)?);
        }
        Ok(b)
    }

    /// The root and components of the folder a walk found.
    fn folder_of(&self, f: Found) -> Result<(usize, Vec<Vec<u8>>), String> {
        if !f.last.is_some_and(|s| s.is_dir()) {
            return Err("not a folder".into());
        }
        let mut below = f.below;
        if let Some(n) = f.name {
            below.push(n.into_bytes());
        }
        Ok((f.root, below))
    }

    fn excluded(&self, st: &Stat) -> bool {
        st.is_dir() && self.excluded.contains(&st.id())
    }

    /// Records a refusal (`what` and the path or program the script named) and gives its errno.
    pub(crate) fn withhold(&self, what: &str, name: &[u8], errno: i32) -> i32 {
        let line = format!("{what} {}", String::from_utf8_lossy(name));
        let mut w = self.withheld.borrow_mut();
        if !w.contains(&line) {
            w.push(line);
        }
        errno
    }

    /// What the script was refused, in the order it asked.
    pub(crate) fn withheld(&self) -> Vec<String> {
        self.withheld.borrow().clone()
    }

    /// The longest root spelling that continues `at` with the next components of `todo` (up to
    /// the first `..`): the root and how many components of `todo` it takes.
    fn root_ahead(&self, at: &At, todo: &std::collections::VecDeque<Vec<u8>>) -> Option<(usize, usize)> {
        let ahead: Vec<&Vec<u8>> = todo.iter().take_while(|c| c.as_slice() != b"..").collect();
        let heres: Vec<Vec<&Vec<u8>>> = match at {
            At::Out(l) => vec![l.iter().collect()],
            At::In { root, below, .. } => self.roots[*root].names.iter().map(|n| n.iter().chain(below.iter()).collect()).collect(),
        };
        let mut best: Option<(usize, usize, usize)> = None;
        for here in &heres {
            for (r, root) in self.roots.iter().enumerate() {
                for n in &root.names {
                    if n.len() <= here.len() || n.len() > here.len() + ahead.len() {
                        continue;
                    }
                    let k = n.len() - here.len();
                    if n.iter().zip(here.iter().chain(ahead[..k].iter())).all(|(a, b)| a == *b) && best.is_none_or(|(_, _, l)| n.len() > l) {
                        best = Some((r, k, n.len()));
                    }
                }
            }
        }
        best.map(|(r, k, _)| (r, k))
    }

    /// Resolves `path` (from the current folder when relative). `follow`: a symlink as its last
    /// component is followed; `write`: the error for a path outside the roots is EACCES.
    fn walk(&self, path: &[u8], follow: bool, write: bool) -> Result<Found, i32> {
        if path.is_empty() {
            return Err(c::ENOENT);
        }
        let what = if write { "write" } else { "read" };
        let outside = if write { c::EACCES } else { c::ENOENT };
        let want_dir = path.ends_with(b"/") || path.ends_with(b"/.");
        let mut todo: std::collections::VecDeque<Vec<u8>> = components(path).into();
        if path[0] != b'/' {
            let Some((r, below)) = &self.cwd else { return Err(self.withhold(what, path, outside)) };
            for c in self.roots[*r].canon.iter().chain(below.iter()).rev() {
                todo.push_front(c.clone());
            }
        }
        let mut at = At::Out(Vec::new());
        let mut links = 0;
        loop {
            if let Some((r, k)) = self.root_ahead(&at, &todo) {
                todo.drain(..k);
                at = At::In { root: r, below: Vec::new(), stack: Vec::new() };
                continue;
            }
            let Some(comp) = todo.pop_front() else { break };
            if comp == b".." {
                match &mut at {
                    At::Out(l) => {
                        l.pop();
                    }
                    At::In { root, below, stack } => {
                        if below.pop().is_some() {
                            stack.pop();
                        } else {
                            // above the root: on from its canonical parent
                            let canon = &self.roots[*root].canon;
                            for c in canon[..canon.len().saturating_sub(1)].iter().rev() {
                                todo.push_front(c.clone());
                            }
                            at = At::Out(Vec::new());
                        }
                    }
                }
                continue;
            }
            let last = todo.is_empty();
            let (root, below, stack) = match &mut at {
                At::Out(l) => {
                    // outside every root and none ahead: only a `..` could lead back into one
                    if !todo.iter().any(|c| c == b"..") {
                        return Err(self.withhold(what, path, outside));
                    }
                    l.push(comp);
                    continue;
                }
                At::In { root, below, stack } => (*root, below, stack),
            };
            let dir = stack.last().map_or(self.roots[root].fd.as_raw_fd(), |f| f.as_raw_fd());
            let name = CString::new(comp.clone()).map_err(|_| c::EINVAL)?;
            let mut st = Stat::zeroed();
            if unsafe { fstatat(dir, name.as_ptr(), st.0.as_mut_ptr(), c::AT_SYMLINK_NOFOLLOW) } != 0 {
                let e = last_errno();
                if e == c::ENOENT && last {
                    let dir = match stack.pop() {
                        Some(f) => DirFd::Own(f),
                        None => DirFd::Root(dir),
                    };
                    return Ok(Found { dir, name: Some(name), root, below: std::mem::take(below), last: None });
                }
                return Err(e);
            }
            if st.is_link() && (!last || follow || want_dir) {
                links += 1;
                if links > c::MAX_LINKS {
                    return Err(c::ELOOP);
                }
                let mut buf = vec![0u8; 4096];
                let n = unsafe { readlinkat(dir, name.as_ptr(), buf.as_mut_ptr(), buf.len()) };
                if n < 0 {
                    return Err(last_errno());
                }
                if n as usize >= buf.len() {
                    return Err(c::ENAMETOOLONG);
                }
                buf.truncate(n as usize);
                if buf.is_empty() {
                    return Err(c::ENOENT);
                }
                for c in components(&buf).into_iter().rev() {
                    todo.push_front(c);
                }
                if buf[0] == b'/' {
                    at = At::Out(Vec::new());
                }
                continue;
            }
            if self.excluded(&st) {
                return Err(self.withhold(what, path, c::ENOENT));
            }
            if last {
                if want_dir && !st.is_dir() {
                    return Err(c::ENOTDIR);
                }
                let dir = match stack.pop() {
                    Some(f) => DirFd::Own(f),
                    None => DirFd::Root(dir),
                };
                return Ok(Found { dir, name: Some(name), root, below: std::mem::take(below), last: Some(st) });
            }
            if !st.is_dir() {
                return Err(c::ENOTDIR);
            }
            let fd = unsafe { openat(dir, name.as_ptr(), c::O_RDONLY | c::O_DIRECTORY | c::O_NOFOLLOW | c::O_CLOEXEC, 0 as std::ffi::c_uint) };
            if fd < 0 {
                return Err(last_errno());
            }
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            // (the folder it opened, whatever happened since the fstatat)
            if self.excluded(&fstat_fd(fd.as_raw_fd())?) {
                return Err(self.withhold(what, path, c::ENOENT));
            }
            stack.push(fd);
            below.push(comp);
        }
        match at {
            At::Out(_) => Err(self.withhold(what, path, outside)),
            At::In { root, below, mut stack } => {
                let dir = match stack.pop() {
                    Some(f) => DirFd::Own(f),
                    None => DirFd::Root(self.roots[root].fd.as_raw_fd()),
                };
                let st = fstat_fd(dir.raw())?;
                Ok(Found { dir, name: None, root, below, last: Some(st) })
            }
        }
    }

    /// A walk for a change: under a write root, of an entry (not a root itself).
    fn walk_to_change(&self, path: &[u8], follow: bool) -> Result<Found, i32> {
        let f = self.walk(path, follow, true)?;
        if !self.roots[f.root].write {
            return Err(self.withhold("write", path, c::EACCES));
        }
        if f.name.is_none() {
            return Err(c::EBUSY);
        }
        Ok(f)
    }

    // ------------------------------------------------------------------ paths

    pub(crate) fn open(&self, path: &[u8], flags: c_int, mode: u32) -> Result<Opened, i32> {
        if path == b"/dev/null" {
            return Ok(Opened::Null);
        }
        let known = c::O_ACCMODE | c::O_CREAT | c::O_EXCL | c::O_TRUNC | c::O_APPEND | c::O_NONBLOCK | c::O_DIRECTORY | c::O_NOFOLLOW | c::O_CLOEXEC | c::O_LARGEFILE;
        if flags & !known != 0 || flags & c::O_ACCMODE == c::O_ACCMODE {
            return Err(c::EINVAL);
        }
        let creates = flags & (c::O_CREAT | c::O_TRUNC) != 0;
        let writes = flags & c::O_ACCMODE != c::O_RDONLY || creates || flags & c::O_APPEND != 0;
        let follow = flags & c::O_NOFOLLOW == 0 && flags & (c::O_CREAT | c::O_EXCL) != c::O_CREAT | c::O_EXCL;
        let f = self.walk(path, follow, writes)?;
        let write = self.roots[f.root].write;
        if writes && !write {
            return Err(self.withhold("write", path, c::EACCES));
        }
        if f.name.is_none() && writes {
            return Err(if creates && flags & c::O_EXCL != 0 { c::EEXIST } else { c::EISDIR });
        }
        let fl = (flags & !c::O_LARGEFILE) | c::O_NOFOLLOW | c::O_CLOEXEC;
        let fd = unsafe { openat(f.dir.raw(), f.at_name().as_ptr(), fl, (mode & 0o7777) as std::ffi::c_uint) };
        if fd < 0 {
            return Err(last_errno());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if self.excluded(&fstat_fd(fd.as_raw_fd())?) {
            return Err(c::ENOENT);
        }
        Ok(Opened::File { fd, write })
    }

    /// `stat` (`follow`) or `lstat` of `path`.
    pub(crate) fn stat(&self, path: &[u8], follow: bool) -> Result<Stat, i32> {
        let f = self.walk(path, follow, false)?;
        f.last.ok_or(c::ENOENT)
    }

    pub(crate) fn mkdir(&self, path: &[u8], mode: u32) -> Result<(), i32> {
        let f = self.walk(path, false, true)?;
        if f.name.is_none() || f.last.is_some() {
            return Err(c::EEXIST);
        }
        if !self.roots[f.root].write {
            return Err(self.withhold("write", path, c::EACCES));
        }
        if unsafe { mkdirat(f.dir.raw(), f.at_name().as_ptr(), (mode & 0o7777) as ModeT) } != 0 {
            return Err(last_errno());
        }
        Ok(())
    }

    /// `unlink`, or `rmdir` (`dir`).
    pub(crate) fn remove(&self, path: &[u8], dir: bool) -> Result<(), i32> {
        let f = self.walk_to_change(path, false)?;
        if unsafe { unlinkat(f.dir.raw(), f.at_name().as_ptr(), if dir { c::AT_REMOVEDIR } else { 0 }) } != 0 {
            return Err(last_errno());
        }
        Ok(())
    }

    pub(crate) fn rename(&self, from: &[u8], to: &[u8]) -> Result<(), i32> {
        let a = self.walk_to_change(from, false)?;
        let b = self.walk_to_change(to, false)?;
        if unsafe { renameat(a.dir.raw(), a.at_name().as_ptr(), b.dir.raw(), b.at_name().as_ptr()) } != 0 {
            return Err(last_errno());
        }
        Ok(())
    }

    /// A symlink's target as it is written (not followed).
    pub(crate) fn readlink(&self, path: &[u8]) -> Result<Vec<u8>, i32> {
        let f = self.walk(path, false, false)?;
        if !f.last.ok_or(c::ENOENT)?.is_link() {
            return Err(c::EINVAL);
        }
        let mut buf = vec![0u8; 4096];
        let n = unsafe { readlinkat(f.dir.raw(), f.at_name().as_ptr(), buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            return Err(last_errno());
        }
        buf.truncate(n as usize);
        Ok(buf)
    }

    /// The path's canonical spelling: its root's canonical one and the components under it.
    pub(crate) fn realpath(&self, path: &[u8]) -> Result<Vec<u8>, i32> {
        let f = self.walk(path, true, false)?;
        if f.last.is_none() {
            return Err(c::ENOENT);
        }
        let mut comps = self.roots[f.root].canon.clone();
        comps.extend(f.below);
        if let Some(n) = f.name {
            comps.push(n.into_bytes());
        }
        Ok(join(&comps))
    }

    pub(crate) fn chmod(&self, path: &[u8], mode: u32) -> Result<(), i32> {
        let f = self.walk(path, true, true)?;
        if !self.roots[f.root].write {
            return Err(self.withhold("write", path, c::EACCES));
        }
        let fd = unsafe { openat(f.dir.raw(), f.at_name().as_ptr(), c::O_RDONLY | c::O_NOFOLLOW | c::O_CLOEXEC, 0 as std::ffi::c_uint) };
        if fd < 0 {
            return Err(last_errno());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if unsafe { fchmod(fd.as_raw_fd(), (mode & 0o7777) as ModeT) } != 0 {
            return Err(last_errno());
        }
        Ok(())
    }

    pub(crate) fn chdir(&mut self, path: &[u8]) -> Result<(), i32> {
        let f = self.walk(path, true, false)?;
        self.cwd = Some(self.folder_of(f).map_err(|_| c::ENOTDIR)?);
        self.virtual_cwd = None;
        Ok(())
    }

    /// The current folder's canonical spelling (a virtual one's, as std's own `current_dir`).
    pub(crate) fn cwd(&self) -> Vec<u8> {
        let Some((r, below)) = &self.cwd else { return self.virtual_cwd.clone().unwrap_or_default().into_bytes() };
        let mut comps = self.roots[*r].canon.clone();
        comps.extend(below.iter().cloned());
        join(&comps)
    }

    /// A folder of `path` to list: the host's `DIR*` (it owns the fd), opened from the walk.
    pub(crate) fn opendir(&self, path: &[u8]) -> Result<usize, i32> {
        let f = self.walk(path, true, false)?;
        if !f.last.ok_or(c::ENOENT)?.is_dir() {
            return Err(c::ENOTDIR);
        }
        let fd = unsafe { openat(f.dir.raw(), f.at_name().as_ptr(), c::O_RDONLY | c::O_DIRECTORY | c::O_NOFOLLOW | c::O_CLOEXEC, 0 as std::ffi::c_uint) };
        if fd < 0 {
            return Err(last_errno());
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        if self.excluded(&fstat_fd(fd.as_raw_fd())?) {
            return Err(c::ENOENT);
        }
        let d = unsafe { fdopendir(fd.as_raw_fd()) };
        if d.is_null() {
            return Err(last_errno());
        }
        // (the listing owns the fd now)
        std::mem::forget(fd);
        Ok(d as usize)
    }

    /// Records a listing: its handle, from 1 (never a host address).
    pub(crate) fn add_dir(&mut self, dir: usize, block: u64) -> u64 {
        let i = match self.dirs.iter().position(|d| d.is_none()) {
            Some(i) => i,
            None => {
                self.dirs.push(None);
                self.dirs.len() - 1
            }
        };
        self.dirs[i] = Some((dir, block));
        i as u64 + 1
    }

    pub(crate) fn dir(&self, h: u64) -> Option<(usize, u64)> {
        usize::try_from(h).ok().and_then(|h| h.checked_sub(1)).and_then(|i| self.dirs.get(i).copied().flatten())
    }

    pub(crate) fn take_dir(&mut self, h: u64) -> Option<(usize, u64)> {
        usize::try_from(h).ok().and_then(|h| h.checked_sub(1)).and_then(|i| self.dirs.get_mut(i)).and_then(|d| d.take())
    }

    /// `path` normalized when it lies in one of the script's store folders; None: the walk's.
    pub(crate) fn store_path(&self, path: &[u8]) -> Option<String> {
        let p = std::str::from_utf8(path).ok()?;
        if !p.starts_with('/') {
            return None;
        }
        let n = crate::vfs::normalize_path(p);
        self.stores.iter().any(|s| crate::vfs::relative_path(&n, s).is_some()).then_some(n)
    }

    // ------------------------------------------------------------------ the fd table

    /// Adds `fd` at the lowest free number from `min`.
    pub(crate) fn add_fd(&mut self, fd: Fd, min: usize) -> i32 {
        let i = match self.fds.iter().skip(min).position(|f| f.is_none()) {
            Some(i) => min + i,
            None => {
                if self.fds.len() < min {
                    self.fds.resize_with(min, || None);
                }
                self.fds.push(None);
                self.fds.len() - 1
            }
        };
        self.fds[i] = Some(fd);
        i as i32
    }

    pub(crate) fn fd(&self, n: i32) -> Option<&Fd> {
        usize::try_from(n).ok().and_then(|i| self.fds.get(i)).and_then(|f| f.as_ref())
    }

    pub(crate) fn take_fd(&mut self, n: i32) -> Option<Fd> {
        usize::try_from(n).ok().and_then(|i| self.fds.get_mut(i)).and_then(|f| f.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn b(p: &Path) -> Vec<u8> {
        p.as_os_str().as_encoded_bytes().to_vec()
    }

    /// The host's own resolution of `p` reads `marker`: the case's path does lead outside, so
    /// a refusal is the walk's check, not a broken path.
    fn host_reads(p: &Path, marker: &str) {
        assert_eq!(std::fs::read_to_string(p).ok().as_deref(), Some(marker), "control: the host resolves {} to the marker", p.display());
    }

    #[test]
    fn the_walk_keeps_a_confined_script_inside_its_folders() {
        let t: PathBuf = std::env::temp_dir().join(format!("relax-broker-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let ws = t.join("ws");
        let out = ws.join("target/debug/build/p-1/out");
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(t.join("outside")).unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(t.join("outside/secret.txt"), "secret").unwrap();
        std::fs::write(ws.join("target/marker.txt"), "target").unwrap();
        std::fs::write(ws.join("src/lib.rs"), "lib").unwrap();
        std::os::unix::fs::symlink("../outside", ws.join("link_out")).unwrap();
        std::os::unix::fs::symlink("target", ws.join("to_target")).unwrap();
        std::os::unix::fs::symlink("loop_b", ws.join("loop_a")).unwrap();
        std::os::unix::fs::symlink("loop_a", ws.join("loop_b")).unwrap();
        // a chain of 41 links to src/lib.rs: chain_0 -> chain_1 -> .. -> chain_40 -> src/lib.rs
        for i in 0..41 {
            let to = if i == 40 { "src/lib.rs".to_string() } else { format!("chain_{}", i + 1) };
            std::os::unix::fs::symlink(to, ws.join(format!("chain_{i}"))).unwrap();
        }
        let spec = Roots { read: vec![b(&ws)], write: vec![b(&out)], excluded: vec![b(&ws.join("target"))], store: Vec::new(), alias: Vec::new(), cwd: b(&ws) };
        let br = Broker::new(&spec).unwrap();
        let read = |p: &[u8]| -> Result<Vec<u8>, i32> {
            match br.open(p, c::O_RDONLY, 0)? {
                Opened::File { fd, .. } => {
                    let mut s = Vec::new();
                    std::io::Read::read_to_end(&mut std::fs::File::from(fd), &mut s).map_err(|e| e.raw_os_error().unwrap_or(0))?;
                    Ok(s)
                }
                Opened::Null => Ok(Vec::new()),
            }
        };
        let joined = |base: &Path, rel: &str| -> Vec<u8> { [b(base), b"/".to_vec(), rel.as_bytes().to_vec()].concat() };

        // inside: relative to the current folder, absolute, and the canonical spelling
        assert_eq!(read(b"src/lib.rs"), Ok(b"lib".to_vec()), "a read of the package's source");
        assert_eq!(read(&joined(&ws, "src/lib.rs")), Ok(b"lib".to_vec()), "an absolute read of the package's source");
        let canon_ws = std::fs::canonicalize(&ws).unwrap();
        assert_eq!(read(&joined(&canon_ws, "src/../src/lib.rs")), Ok(b"lib".to_vec()), "the canonical spelling, with a `..` inside the root");
        assert_eq!(br.realpath(&b(&out)), Ok(b(&std::fs::canonicalize(&out).unwrap())), "realpath(OUT_DIR) is OUT_DIR's canonical path");

        // dot-dot out of the root
        let p = ws.join("src/../../outside/secret.txt");
        host_reads(&p, "secret");
        assert_eq!(read(&b(&p)), Err(c::ENOENT), "`..` past the source root");
        assert_eq!(read(b"../outside/secret.txt"), Err(c::ENOENT), "`..` past the current folder's root");

        // a link out
        let p = ws.join("link_out/secret.txt");
        host_reads(&p, "secret");
        assert_eq!(read(&b(&p)), Err(c::ENOENT), "a symlink in the root that points outside");
        assert_eq!(br.readlink(b"link_out"), Ok(b"../outside".to_vec()), "the link itself reads as written");

        // the excluded folder: directly, through a link, and up from OUT_DIR
        for (p, what) in [(ws.join("target/marker.txt"), "directly"), (ws.join("to_target/marker.txt"), "through a workspace symlink"), (out.join("../../../../marker.txt"), "up from OUT_DIR")] {
            host_reads(&p, "target");
            assert_eq!(read(&b(&p)), Err(c::ENOENT), "the target folder {what}");
        }
        assert_eq!(br.stat(&b(&ws.join("target")), true).err(), Some(c::ENOENT), "a stat of the target folder");

        // OUT_DIR inside it is its own root: written, read, and reached by `..` from inside
        match br.open(&joined(&out, "gen.rs"), c::O_WRONLY | c::O_CREAT | c::O_TRUNC, 0o644).unwrap() {
            Opened::File { fd, write } => {
                assert!(write);
                std::io::Write::write_all(&mut std::fs::File::from(fd), b"gen").unwrap();
            }
            Opened::Null => panic!("a file in OUT_DIR opened as the null device"),
        }
        assert_eq!(read(&joined(&out, "../out/gen.rs")), Ok(b"gen".to_vec()), "OUT_DIR/../out stays in OUT_DIR");
        assert_eq!(br.mkdir(&joined(&out, "sub"), 0o755), Ok(()));
        assert_eq!(br.rename(&joined(&out, "gen.rs"), &joined(&out, "sub/gen.rs")), Ok(()));

        // a write outside the write root
        assert_eq!(br.open(b"src/new.rs", c::O_WRONLY | c::O_CREAT, 0o644).err(), Some(c::EACCES), "a write into the read root");
        assert_eq!(br.mkdir(b"newdir", 0o755), Err(c::EACCES));

        // a rename out of OUT_DIR
        let p = t.join("outside/moved.rs");
        assert_eq!(br.rename(&joined(&out, "sub/gen.rs"), &b(&p)), Err(c::EACCES), "a rename out of OUT_DIR");
        assert_eq!(br.rename(&joined(&out, "sub/gen.rs"), &joined(&out, "../../../../moved.rs")), Err(c::ENOENT), "a rename into the target folder");
        std::fs::rename(out.join("sub/gen.rs"), &p).unwrap();
        host_reads(&p, "gen");

        // links: a loop, a chain of 40 (followed), a chain of 41 (refused)
        assert_eq!(read(b"loop_a"), Err(c::ELOOP), "a link loop");
        assert_eq!(read(b"chain_1"), Ok(b"lib".to_vec()), "control: 40 links resolve");
        assert_eq!(read(b"chain_0"), Err(c::ELOOP), "41 links");

        // the current folder moves within the roots only
        let mut br = br;
        assert_eq!(br.chdir(&b(&out)), Ok(()));
        assert_eq!(br.cwd(), b(&std::fs::canonicalize(&out).unwrap()));
        assert_eq!(br.chdir(&b(&t.join("outside"))), Err(c::ENOENT));
        let _ = std::fs::remove_dir_all(&t);
    }
}
