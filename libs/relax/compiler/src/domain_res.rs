//! What a run holds of the OS, so that its end gives it back in whatever state the run died:
//! files, sockets, pipes, directories, mappings, signal handlers, thread-local keys, fork handlers
//! (Windows: handles, sockets, views, reservations, wait objects). The program reaches these
//! through the interposers here (jit.rs `lookup_interposer`), which see only the calls that create
//! or give back a resource: a read or a write never comes here.
//!
//! An fd number can be given back by native code this table never hears of: its entry names what
//! it was opened as, and one that is something else by now is dropped, not closed.
//!
//! A process a run started ends with it, so that restarts do not pile up a second hub or helper
//! each. A process meant to outlive the run says so the way the OS has for it: its own session
//! on unix (`setsid` before exec), DETACHED_PROCESS on Windows. It only goes down the tree as far
//! as the child's process group (unix) or job (Windows): a grandchild in neither stays.
//!
//! The process's own domain (0) is not recorded: it has no end before the process's.
//!
//! The table is keyed by what each record is (its kind and its number: an fd, a handle, a
//! mapping's address, a TLS key), so an interposed create costs one insert and a close one
//! remove, never a walk; a run's end walks it once. Each record carries the order it was made in,
//! and a run's end gives back in the reverse of that.
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// What a run's end gave back (`domain::RunEnd`). The same fields on every platform; a pipe, a
/// directory and any other kernel object count as files.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Released {
    pub files: u32,
    pub sockets: u32,
    pub mappings: u32,
    pub mapped_bytes: u64,
    /// signal handlers, thread-local keys and fork handlers: they would have called its code
    pub callbacks: u32,
    /// processes it started and left running, ended with it
    pub children_ended: u32,
    /// processes it started to outlive it (`detached`): left alone
    pub children_detached: u32,
    /// processes it started that a kill did not end yet: tried again at the next end
    pub children_left: u32,
}

impl Released {
    /// The part of the run's ledger line, empty when there was nothing.
    pub fn line(&self) -> String {
        let mut l = String::new();
        if self.files != 0 || self.sockets != 0 {
            l += &format!("; closed {} files, {} sockets", self.files, self.sockets);
        }
        if self.mappings != 0 {
            l += &format!("; unmapped {} ({} KB)", self.mappings, self.mapped_bytes / 1024);
        }
        if self.callbacks != 0 {
            l += &format!("; {} callbacks taken back", self.callbacks);
        }
        if self.children_ended != 0 {
            l += &format!("; ended {} child processes", self.children_ended);
        }
        if self.children_detached != 0 {
            l += &format!("; {} detached processes stay", self.children_detached);
        }
        if self.children_left != 0 {
            l += &format!("; {} child processes not gone yet", self.children_left);
        }
        l
    }
}

#[cfg(unix)]
fn pages(len: usize) -> usize {
    let p = crate::os::mem::page_size() as usize;
    (len + p - 1) & !(p - 1)
}

/// The domain the calling thread's resources belong to: a run's, or the main program's once it
/// asked for that (`relax_track_handles`).
fn owner() -> Option<u64> {
    let base = crate::domain::current();
    (base != 0 && (base != crate::domain::default_base() || RECORD_MAIN.load(std::sync::atomic::Ordering::Relaxed))).then_some(base)
}

/// The main program's resources are recorded too (a run's always are): from its
/// `relax_track_handles` on. No run's end gives them back; its own closes forget them.
static RECORD_MAIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Runtime introspection, for a program to look up through its loader (dlsym; GetProcAddress on
/// its own module): from now on the runtime records what the main program holds as it does a
/// run's. Each of its interposed creates and closes then keeps the table as a run's do (an insert
/// at a create, a remove at a close); before this, the main program pays one atomic load.
pub(crate) extern "C" fn relax_track_handles() {
    make_room();
    RECORD_MAIN.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Runtime introspection, looked up as `relax_track_handles` is: the handles (Windows) or
/// descriptors (unix) the runtime records the program holding, its own handles to the program's
/// children among them. Writes up to `cap` of them to `buf` and returns how many there are; with
/// the main program not tracked (no `relax_track_handles` yet), `usize::MAX`: the list would leave
/// out all it holds. Only what goes through a creator the runtime interposes is recorded: what
/// the program makes otherwise, a run's end cannot give back either. In the order they were made.
pub(crate) extern "C" fn relax_held_handles(buf: *mut u64, cap: usize) -> usize {
    if !RECORD_MAIN.load(std::sync::atomic::Ordering::Relaxed) {
        return usize::MAX;
    }
    let mut held: Vec<(u64, u64)> = with_table(|t| t.all().filter_map(|o| held_handle(&o.res).map(|h| (o.seq, h))).collect());
    held.sort_unstable();
    for (i, &(_, h)) in held.iter().take(cap).enumerate() {
        if !buf.is_null() {
            // SAFETY: the program passes room for `cap` values
            unsafe { *buf.add(i) = h };
        }
    }
    held.len()
}

/// An atfork child: the table's lock may be held for good by a thread that did not come along.
static FORKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// In the table on this thread: a signal handler's `close` (async-signal-safe, so a program
    /// may call it there) must not wait for the lock it interrupted.
    static IN_TABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` on the table, unless this is a place that may not wait for it (nothing is recorded
/// then: a stale entry is dropped at the end, a missing one is the one thing lost). `f` is an
/// insert, a remove or a look: no system call is made under the table's lock.
fn with_table<R: Default>(f: impl FnOnce(&mut Table) -> R) -> R {
    if FORKED.load(std::sync::atomic::Ordering::Relaxed) || IN_TABLE.with(|t| t.replace(true)) {
        return R::default();
    }
    let r = crate::jit::host_boundary("run resource table", || {
        let _kept = crate::zone_alloc::kept();
        f(&mut OWNED.lock().unwrap_or_else(|e| e.into_inner()))
    });
    IN_TABLE.with(|t| t.set(false));
    r
}

fn note(res: Res) {
    if let Some(domain) = owner() {
        // A program's signal handler may call an interposed create (open, socket, dup, pipe,
        // accept, fork are async-signal-safe). One that interrupted this thread in the table finds
        // IN_TABLE set and records nothing, so it never inserts under the lock it interrupted. One
        // that interrupted the allocator would wait in it if its insert grew a map: inside a
        // handler an insert only takes the map's spare room, which the inserts outside handlers
        // keep topped up (`SPARE`), and a record that finds none is left out and counted.
        #[cfg(unix)]
        let in_handler = crate::os::progsig::in_program_handler();
        #[cfg(not(unix))]
        let in_handler = false;
        if in_handler {
            HANDLER_RECORDS.fetch_add(1, Ordering::Relaxed);
        }
        match with_table(|t| t.insert(domain, res, in_handler)) {
            Inserted::New => {}
            Inserted::Replaced(kind) => replaced(kind),
            Inserted::LeftOut(kind) => left_out(kind),
            // a handler that interrupted its thread inside the table (a fork's child records
            // nothing, by design)
            Inserted::NotReached => {
                if in_handler && !FORKED.load(Ordering::Relaxed) {
                    left_out(kind_of(&res));
                }
            }
        }
    }
}

/// What an insert did.
#[derive(Default)]
enum Inserted {
    /// (`with_table` did not reach the table)
    #[default]
    NotReached,
    New,
    /// it replaced a record under the same key (`replaced`)
    Replaced(Kind),
    /// a program's signal handler made it, and the map had no spare room for it (`left_out`)
    LeftOut(Kind),
}

/// The room the inserts outside handlers keep free in each HashMap for the inserts a program's
/// signal handler makes, which never grow a map: when an insert leaves less than this, it
/// reserves twice this more (one rehash per this many inserts at the least, amortized as growth
/// is). A handler that finds none leaves its record out (`left_out`): the open still succeeds.
const SPARE: usize = 64;

fn kind_of(res: &Res) -> Kind {
    match place(res) {
        Place::Key(kind, _) => kind,
        Place::Child(_) => Kind::Child,
        #[cfg(unix)]
        Place::Map(_) => Kind::Map,
    }
}

/// May `map` take `key` without growing (an insert of a new key with no room left grows it)?
fn has_room<K: std::hash::Hash + Eq, V>(map: &std::collections::HashMap<K, V, Fixed>, key: &K) -> bool {
    map.len() < map.capacity() || map.contains_key(key)
}

/// Outside handlers: keeps `SPARE` free in `map`.
fn top_up<K: std::hash::Hash + Eq, V>(map: &mut std::collections::HashMap<K, V, Fixed>) {
    if map.capacity() - map.len() < SPARE {
        map.reserve(2 * SPARE);
    }
}

/// Before a run's code starts (`domain::create`; the main program's, at `relax_track_handles`):
/// the maps get their spare room, so a create in a handler before any create outside one finds it.
pub(crate) fn make_room() {
    with_table(|t| {
        top_up(&mut t.held);
        top_up(&mut t.children);
    });
}

#[derive(Clone, Copy)]
struct Owned {
    domain: u64,
    /// the order it was made in (`Table::seq`)
    seq: u64,
    res: Res,
}

/// A fixed hasher: the map is a static (no random keys to make), and its keys are the system's
/// numbers, not anything a program picks to collide.
type Fixed = std::hash::BuildHasherDefault<std::hash::DefaultHasher>;

/// What the runs hold. A remove from a HashMap frees nothing (it never shrinks), so a close in a
/// signal handler (close and waitpid are async-signal-safe) does no allocator call.
struct Table {
    /// the last record's `Owned::seq`
    seq: u64,
    /// by kind and number
    held: std::collections::HashMap<(Kind, u64), Owned, Fixed>,
    /// the processes the runs started, by pid
    children: std::collections::HashMap<u64, Owned, Fixed>,
    /// unix: the mappings, by start address (an unmap takes a range, a part of one or several)
    #[cfg(unix)]
    maps: std::collections::BTreeMap<usize, Owned>,
}

/// Where a record goes in the table.
enum Place {
    Key(Kind, u64),
    Child(u64),
    #[cfg(unix)]
    Map(usize),
}

impl Table {
    const fn new() -> Table {
        Table {
            seq: 0,
            held: std::collections::HashMap::with_hasher(Fixed::new()),
            children: std::collections::HashMap::with_hasher(Fixed::new()),
            #[cfg(unix)]
            maps: std::collections::BTreeMap::new(),
        }
    }

    /// Records `res` for `domain`. Replaced: a record was there under the same key, so the system
    /// gave that number out again and the earlier one was closed by a path the runtime does not
    /// see. `in_handler` (a program's signal handler made it): the insert allocates nothing, it
    /// takes the map's spare room or is left out; a mapping always is (a BTreeMap insert may
    /// allocate a node; mmap is not async-signal-safe).
    fn insert(&mut self, domain: u64, res: Res, in_handler: bool) -> Inserted {
        let o = Owned { domain, seq: self.seq + 1, res };
        let replaced = match place(&res) {
            Place::Key(kind, n) => {
                if in_handler && !has_room(&self.held, &(kind, n)) {
                    return Inserted::LeftOut(kind);
                }
                let old = self.held.insert((kind, n), o).map(|_| kind);
                if !in_handler {
                    top_up(&mut self.held);
                }
                old
            }
            Place::Child(pid) => {
                if in_handler && !has_room(&self.children, &pid) {
                    return Inserted::LeftOut(Kind::Child);
                }
                let old = self.children.insert(pid, o).map(|_| Kind::Child);
                if !in_handler {
                    top_up(&mut self.children);
                }
                old
            }
            #[cfg(unix)]
            Place::Map(addr) => {
                if in_handler {
                    return Inserted::LeftOut(Kind::Map);
                }
                self.maps.insert(addr, o);
                None
            }
        };
        self.seq += 1;
        replaced.map_or(Inserted::New, Inserted::Replaced)
    }

    fn remove(&mut self, kind: Kind, n: u64) -> Option<Owned> {
        self.held.remove(&(kind, n))
    }

    fn all(&self) -> impl Iterator<Item = &Owned> {
        let held = self.held.values().chain(self.children.values());
        #[cfg(unix)]
        let held = held.chain(self.maps.values());
        held
    }

    /// Takes out every record `of` says, in no order.
    fn take(&mut self, of: impl Fn(&Owned) -> bool) -> Vec<Owned> {
        let mut out = Vec::new();
        let mut keep = |o: &mut Owned| {
            if of(o) {
                out.push(*o);
                false
            } else {
                true
            }
        };
        self.held.retain(|_, o| keep(o));
        self.children.retain(|_, o| keep(o));
        #[cfg(unix)]
        self.maps.retain(|_, o| keep(o));
        out
    }
}

static OWNED: Mutex<Table> = Mutex::new(Table::new());

/// Records replaced under their key, by kind (`Kind as usize`).
static REPLACED: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// The kinds whose replacement was said (a bit each).
static REPLACED_SAID: AtomicU32 = AtomicU32::new(0);

/// A record replaced: counted, and said once per kind. It names a close the runtime does not
/// interpose (native code's own close, a call it does not bind): what to interpose next.
fn replaced(kind: Kind) {
    let i = kind as usize;
    REPLACED[i].fetch_add(1, Ordering::Relaxed);
    if REPLACED_SAID.fetch_or(1 << i, Ordering::Relaxed) & (1 << i) == 0 {
        say(&["relax: a run's ", kind.name(), " was given out again while the runtime still recorded it: it was closed by a path the runtime does not see (counted; said once per kind)\n"]);
    }
}

/// Records a program's signal handlers made that found no spare room (`SPARE`), all kinds.
static LEFT_OUT: AtomicU64 = AtomicU64::new(0);
static LEFT_OUT_SAID: AtomicU32 = AtomicU32::new(0);
/// Records a program's signal handlers made, left out or not.
static HANDLER_RECORDS: AtomicU64 = AtomicU64::new(0);

/// A record a program's signal handler made, left out: counted, and said once. The run's end does
/// not give that resource back; the create itself succeeded.
fn left_out(kind: Kind) {
    LEFT_OUT.fetch_add(1, Ordering::Relaxed);
    if LEFT_OUT_SAID.swap(1, Ordering::Relaxed) == 0 {
        say(&["relax: a program's signal handler made a ", kind.name(), " while the runs' resource table had no spare room, so it is not recorded: the run's end does not give it back (counted; said once)\n"]);
    }
}

/// How many records a program's signal handlers made, and how many of them were left out.
#[cfg(all(test, unix))]
pub(crate) fn handler_records() -> (u64, u64) {
    (HANDLER_RECORDS.load(Ordering::Relaxed), LEFT_OUT.load(Ordering::Relaxed))
}

/// A line to stderr from anywhere, a program's signal handler included: on unix written straight
/// to fd 2 (std's stderr takes a lock and a RefCell a handler may have interrupted).
fn say(parts: &[&str]) {
    #[cfg(unix)]
    {
        extern "C" {
            fn write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
        }
        for p in parts {
            unsafe { write(2, p.as_ptr().cast(), p.len()) };
        }
    }
    #[cfg(not(unix))]
    eprint!("{}", parts.concat());
}

/// How many records were replaced under their key so far, all kinds.
#[cfg(test)]
pub(crate) fn replaced_records() -> u64 {
    REPLACED.iter().map(|c| c.load(Ordering::Relaxed)).sum()
}

/// The handlers a run registered with `pthread_atfork`: only its own `fork` runs them (nothing
/// is left with libc to call its code after its end).
#[cfg(unix)]
type ForkFn = Option<unsafe extern "C" fn()>;
#[cfg(unix)]
static ATFORK: Mutex<Vec<(u64, ForkFn, ForkFn, ForkFn)>> = Mutex::new(Vec::new());

/// `ATFORK` was used once: an exec child must not lock the registry it may have inherited locked.
/// Conservative, never cleared while the run lives, even when a domain's handlers go; a run started
/// again in place clears it with the registry (reset_statics).
#[cfg(unix)]
static ATFORK_USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The run may have registered fork handlers (an exec child's fork then takes the full path).
#[cfg(unix)]
pub(crate) fn atfork_may_have_handlers() -> bool {
    ATFORK_USED.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(unix)]
use unix::{Kind, Res};
#[cfg(unix)]
pub(crate) use unix::*;
#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{c_char, c_int, c_uint, c_void};
    use std::os::fd::FromRawFd;

    #[derive(Clone, Copy)]
    pub(super) enum Res {
        Fd { fd: c_int, dev: u64, ino: u64, socket: bool },
        Dir(usize),
        Map { addr: usize, len: usize },
        Key(KeyT),
        Child { pid: i32, told: bool, detached: bool },
    }

    /// What a record's number is a number of (`Table::held`'s key with it).
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub(super) enum Kind {
        Fd,
        Dir,
        Key,
        Child,
        /// (`Table::maps`, keyed by address: named here only, for what a record was)
        Map,
    }

    impl Kind {
        pub(super) fn name(self) -> &'static str {
            match self {
                Kind::Fd => "file descriptor",
                Kind::Dir => "directory stream",
                Kind::Key => "thread-local key",
                Kind::Child => "child process",
                Kind::Map => "mapping",
            }
        }
    }

    pub(super) fn place(res: &Res) -> Place {
        match *res {
            Res::Fd { fd, .. } => Place::Key(Kind::Fd, fd as u64),
            Res::Dir(dir) => Place::Key(Kind::Dir, dir as u64),
            Res::Key(k) => Place::Key(Kind::Key, k as u64),
            Res::Map { addr, .. } => Place::Map(addr),
            Res::Child { pid, .. } => Place::Child(pid as u64),
        }
    }

    /// What would call the run's code from threads not its own (`take_back_callbacks`).
    pub(super) fn is_callback(res: &Res) -> bool {
        matches!(res, Res::Key(_))
    }

    /// The descriptor a record holds open, for `relax_held_handles`.
    pub(super) fn held_handle(res: &Res) -> Option<u64> {
        match *res {
            Res::Fd { fd, .. } => Some(fd as u64),
            Res::Dir(dir) => {
                let fd = unsafe { sys::dirfd(dir as *mut c_void) };
                (fd >= 0).then_some(fd as u64)
            }
            Res::Map { .. } | Res::Key(_) | Res::Child { .. } => None,
        }
    }

    /// pthread_key_t
    #[cfg(target_vendor = "apple")]
    pub type KeyT = usize;
    #[cfg(not(target_vendor = "apple"))]
    pub type KeyT = u32;

    /// What number `fd` is now.
    fn identity(fd: c_int) -> Option<(u64, u64, bool)> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd) });
        let m = file.metadata().ok()?;
        Some((m.dev(), m.ino(), m.file_type().is_socket()))
    }

    fn note_fd(fd: c_int) -> c_int {
        // (stdin, stdout and stderr are the process's, whatever the program put there)
        if fd > 2 {
            if let Some((dev, ino, socket)) = identity(fd) {
                note(Res::Fd { fd, dev, ino, socket });
            }
        }
        fd
    }

    fn forget_fd(fd: c_int) {
        with_table(|t| {
            t.remove(Kind::Fd, fd as u64);
        });
    }

    /// The range is no longer the run's: what is left of a mapping around it stays. The run's
    /// mappings do not overlap (a mapping over one forgets it first), so only the one that starts
    /// below the range can reach into it; the others start in it.
    fn forget_range(addr: usize, len: usize) {
        let end = addr + len;
        with_table(|t| {
            let below = t.maps.range(..addr).next_back().filter(|&(&a, o)| matches!(o.res, Res::Map { len: l, .. } if a + l > addr)).map(|(&a, _)| a);
            let hit: Vec<usize> = below.into_iter().chain(t.maps.range(addr..end).map(|(&a, _)| a)).collect();
            for a in hit {
                let Some(o) = t.maps.remove(&a) else { continue };
                let Res::Map { len: l, .. } = o.res else { continue };
                let e = a + l;
                if a < addr {
                    t.maps.insert(a, Owned { res: Res::Map { addr: a, len: addr - a }, ..o });
                }
                if end < e {
                    t.maps.insert(end, Owned { res: Res::Map { addr: end, len: e - end }, ..o });
                }
            }
        });
    }

    mod sys {
        use super::*;
        extern "C" {
            pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
            pub fn openat(dir: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
            pub fn creat(path: *const c_char, mode: c_uint) -> c_int;
            pub fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
            pub fn socketpair(domain: c_int, ty: c_int, protocol: c_int, fds: *mut c_int) -> c_int;
            pub fn accept(fd: c_int, addr: *mut c_void, len: *mut c_uint) -> c_int;
            pub fn pipe(fds: *mut c_int) -> c_int;
            pub fn dup(fd: c_int) -> c_int;
            pub fn dup2(from: c_int, to: c_int) -> c_int;
            pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
            pub fn mkstemp(template: *mut c_char) -> c_int;
            pub fn close(fd: c_int) -> c_int;
            pub fn opendir(path: *const c_char) -> *mut c_void;
            pub fn fdopendir(fd: c_int) -> *mut c_void;
            pub fn closedir(dir: *mut c_void) -> c_int;
            pub fn dirfd(dir: *mut c_void) -> c_int;
            pub fn mmap(addr: *mut u8, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut u8;
            pub fn munmap(addr: *mut u8, len: usize) -> c_int;
            pub fn pthread_key_create(key: *mut usize, dtor: Option<extern "C" fn(*mut u8)>) -> c_int;
            pub fn pthread_key_delete(key: KeyT) -> c_int;
            pub fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int;
            pub fn posix_spawnp(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int;
            pub fn waitpid(pid: i32, status: *mut c_int, options: c_int) -> i32;
            pub fn kill(pid: i32, sig: c_int) -> c_int;
            pub fn getpgid(pid: i32) -> i32;
            pub fn getsid(pid: i32) -> i32;
        }
        #[cfg(any(target_os = "linux", target_os = "android"))]
        extern "C" {
            pub fn accept4(fd: c_int, addr: *mut c_void, len: *mut c_uint, flags: c_int) -> c_int;
            pub fn pipe2(fds: *mut c_int, flags: c_int) -> c_int;
            pub fn dup3(from: c_int, to: c_int, flags: c_int) -> c_int;
            pub fn eventfd(init: c_uint, flags: c_int) -> c_int;
            pub fn inotify_init1(flags: c_int) -> c_int;
            pub fn epoll_create1(flags: c_int) -> c_int;
            pub fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
            pub fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
            pub fn mremap(old: *mut c_void, old_len: usize, new_len: usize, flags: c_int, ...) -> *mut c_void;
            pub fn syscall(n: std::ffi::c_long, ...) -> std::ffi::c_long;
        }
        #[cfg(target_vendor = "apple")]
        extern "C" {
            pub fn kqueue() -> c_int;
        }
    }

    // The variadic ones (open, openat, fcntl): the next argument is in the next register, except
    // on Apple's arm64, where variadic arguments go on the stack, past the eight argument
    // registers.
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::open(path, flags, mode) })
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn open(path: *const c_char, flags: c_int, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::open(path, flags, mode) })
    }
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn openat(dir: c_int, path: *const c_char, flags: c_int, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::openat(dir, path, flags, mode) })
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn openat(dir: c_int, path: *const c_char, flags: c_int, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::openat(dir, path, flags, mode) })
    }
    fn fcntl_inner(fd: c_int, cmd: c_int, arg: usize) -> c_int {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        const DUP_CLOEXEC: c_int = 1030;
        #[cfg(target_vendor = "apple")]
        const DUP_CLOEXEC: c_int = 67;
        const F_DUPFD: c_int = 0;
        let r = unsafe { sys::fcntl(fd, cmd, arg) };
        if (cmd == F_DUPFD || cmd == DUP_CLOEXEC) && r >= 0 {
            note_fd(r);
        }
        r
    }
    #[cfg(not(all(target_vendor = "apple", target_arch = "aarch64")))]
    pub extern "C" fn fcntl(fd: c_int, cmd: c_int, arg: usize) -> c_int {
        fcntl_inner(fd, cmd, arg)
    }
    #[cfg(all(target_vendor = "apple", target_arch = "aarch64"))]
    pub extern "C" fn fcntl(fd: c_int, cmd: c_int, _2: u64, _3: u64, _4: u64, _5: u64, _6: u64, _7: u64, arg: usize) -> c_int {
        fcntl_inner(fd, cmd, arg)
    }

    pub extern "C" fn creat(path: *const c_char, mode: c_uint) -> c_int {
        note_fd(unsafe { sys::creat(path, mode) })
    }
    pub extern "C" fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int {
        note_fd(unsafe { sys::socket(domain, ty, protocol) })
    }
    fn note_two(r: c_int, fds: *mut c_int) -> c_int {
        if r == 0 {
            unsafe {
                note_fd(*fds);
                note_fd(*fds.add(1));
            }
        }
        r
    }
    pub extern "C" fn socketpair(domain: c_int, ty: c_int, protocol: c_int, fds: *mut c_int) -> c_int {
        note_two(unsafe { sys::socketpair(domain, ty, protocol, fds) }, fds)
    }
    pub extern "C" fn accept(fd: c_int, addr: *mut c_void, len: *mut c_uint) -> c_int {
        note_fd(unsafe { sys::accept(fd, addr, len) })
    }
    pub extern "C" fn pipe(fds: *mut c_int) -> c_int {
        note_two(unsafe { sys::pipe(fds) }, fds)
    }
    pub extern "C" fn dup(fd: c_int) -> c_int {
        note_fd(unsafe { sys::dup(fd) })
    }
    pub extern "C" fn dup2(from: c_int, to: c_int) -> c_int {
        forget_fd(to);
        let r = unsafe { sys::dup2(from, to) };
        if r >= 0 { note_fd(r) } else { r }
    }
    pub extern "C" fn mkstemp(template: *mut c_char) -> c_int {
        note_fd(unsafe { sys::mkstemp(template) })
    }
    pub extern "C" fn close(fd: c_int) -> c_int {
        // (first: once it is closed, another thread can get this number)
        forget_fd(fd);
        unsafe { sys::close(fd) }
    }
    pub extern "C" fn opendir(path: *const c_char) -> *mut c_void {
        let d = unsafe { sys::opendir(path) };
        if !d.is_null() {
            note(Res::Dir(d as usize));
        }
        d
    }
    pub extern "C" fn fdopendir(fd: c_int) -> *mut c_void {
        let d = unsafe { sys::fdopendir(fd) };
        if !d.is_null() {
            forget_fd(fd);
            note(Res::Dir(d as usize));
        }
        d
    }
    pub extern "C" fn closedir(d: *mut c_void) -> c_int {
        with_table(|t| {
            t.remove(Kind::Dir, d as u64);
        });
        unsafe { sys::closedir(d) }
    }
    pub extern "C" fn mmap(addr: *mut u8, len: usize, prot: c_int, flags: c_int, fd: c_int, off: i64) -> *mut u8 {
        let p = unsafe { sys::mmap(addr, len, prot, flags, fd, off) };
        if p as isize != -1 {
            let len = pages(len);
            // (MAP_FIXED over what it had)
            forget_range(p as usize, len);
            note(Res::Map { addr: p as usize, len });
        }
        p
    }
    pub extern "C" fn munmap(addr: *mut u8, len: usize) -> c_int {
        forget_range(addr as usize, pages(len));
        unsafe { sys::munmap(addr, len) }
    }
    // the program's handler goes in a table and the kernel runs a trampoline for it (os/progsig.rs:
    // never inside a compile); the run's end gives back what its domain changed
    pub extern "C" fn signal(sig: c_int, handler: usize) -> usize {
        unsafe { crate::os::progsig::signal(sig, handler, owner().unwrap_or(0)) }
    }
    pub extern "C" fn sigaction(sig: c_int, act: *const crate::os::fault::SigAction, old: *mut crate::os::fault::SigAction) -> c_int {
        unsafe { crate::os::progsig::sigaction(sig, act.cast(), old.cast(), owner().unwrap_or(0)) }
    }
    pub extern "C" fn pthread_key_create(key: *mut KeyT, dtor: Option<extern "C" fn(*mut u8)>) -> c_int {
        let mut k = 0usize;
        let r = unsafe { sys::pthread_key_create(&mut k, dtor) };
        if r == 0 {
            unsafe { *key = k as KeyT };
            note(Res::Key(k as KeyT));
        }
        r
    }
    pub extern "C" fn pthread_key_delete(key: KeyT) -> c_int {
        with_table(|t| {
            t.remove(Kind::Key, key as u64);
        });
        unsafe { sys::pthread_key_delete(key) }
    }
    pub(crate) fn note_child(pid: i32) {
        if pid > 0 {
            crate::jit::note_progress(crate::jit::PROGRESS_PROC);
            note(Res::Child { pid, told: false, detached: false });
        }
    }
    pub extern "C" fn posix_spawn(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int {
        let r = unsafe { sys::posix_spawn(pid, path, actions, attr, argv, envp) };
        if r == 0 && !pid.is_null() {
            note_child(unsafe { *pid });
        }
        r
    }
    pub extern "C" fn posix_spawnp(pid: *mut i32, path: *const c_char, actions: *const c_void, attr: *const c_void, argv: *const usize, envp: *const usize) -> c_int {
        let r = unsafe { sys::posix_spawnp(pid, path, actions, attr, argv, envp) };
        if r == 0 && !pid.is_null() {
            note_child(unsafe { *pid });
        }
        r
    }
    pub extern "C" fn waitpid(pid: i32, status: *mut c_int, options: c_int) -> i32 {
        let r = unsafe { sys::waitpid(pid, status, options) };
        if r > 0 {
            with_table(|t| {
                t.children.remove(&(r as u64));
            });
        }
        r
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod linux {
        use super::*;
        use std::ffi::c_long;
        pub extern "C" fn accept4(fd: c_int, addr: *mut c_void, len: *mut c_uint, flags: c_int) -> c_int {
            note_fd(unsafe { sys::accept4(fd, addr, len, flags) })
        }
        pub extern "C" fn pipe2(fds: *mut c_int, flags: c_int) -> c_int {
            note_two(unsafe { sys::pipe2(fds, flags) }, fds)
        }
        pub extern "C" fn dup3(from: c_int, to: c_int, flags: c_int) -> c_int {
            forget_fd(to);
            let r = unsafe { sys::dup3(from, to, flags) };
            if r >= 0 { note_fd(r) } else { r }
        }
        pub extern "C" fn eventfd(init: c_uint, flags: c_int) -> c_int {
            note_fd(unsafe { sys::eventfd(init, flags) })
        }
        pub extern "C" fn inotify_init1(flags: c_int) -> c_int {
            note_fd(unsafe { sys::inotify_init1(flags) })
        }
        pub extern "C" fn epoll_create1(flags: c_int) -> c_int {
            note_fd(unsafe { sys::epoll_create1(flags) })
        }
        pub extern "C" fn timerfd_create(clock: c_int, flags: c_int) -> c_int {
            note_fd(unsafe { sys::timerfd_create(clock, flags) })
        }
        pub extern "C" fn memfd_create(name: *const c_char, flags: c_uint) -> c_int {
            note_fd(unsafe { sys::memfd_create(name, flags) })
        }
        pub extern "C" fn mremap(old: *mut c_void, old_len: usize, new_len: usize, flags: c_int, at: usize) -> *mut c_void {
            let p = unsafe { sys::mremap(old, old_len, new_len, flags, at) };
            if p as isize != -1 {
                forget_range(old as usize, pages(old_len));
                forget_range(p as usize, pages(new_len));
                note(Res::Map { addr: p as usize, len: pages(new_len) });
            }
            p
        }
        /// The fds that come from a number, not a name (a thread's perf counters). Everything else
        /// goes straight through: most of what passes here is a futex.
        pub extern "C" fn syscall(n: c_long, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize) -> c_long {
            #[cfg(target_arch = "x86_64")]
            const SYS_PERF_EVENT_OPEN: c_long = 298;
            #[cfg(target_arch = "aarch64")]
            const SYS_PERF_EVENT_OPEN: c_long = 241;
            let r = unsafe { sys::syscall(n, a, b, c, d, e, f) };
            if n == SYS_PERF_EVENT_OPEN && r >= 0 {
                note_fd(r as c_int);
            }
            r
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    use linux::*;
    #[cfg(target_vendor = "apple")]
    pub extern "C" fn kqueue() -> c_int {
        note_fd(unsafe { sys::kqueue() })
    }

    /// The run's address for a C symbol it binds (jit.rs `lookup_interposer`).
    pub(crate) fn interposer(link: &str) -> Option<u64> {
        macro_rules! at {
            ($f:expr) => { Some($f as *const () as usize as u64) };
        }
        match link {
            "open" | "open64" => at!(open),
            "openat" | "openat64" => at!(openat),
            "creat" | "creat64" => at!(creat),
            "socket" => at!(socket),
            "socketpair" => at!(socketpair),
            "accept" => at!(accept),
            "pipe" => at!(pipe),
            "dup" => at!(dup),
            "dup2" => at!(dup2),
            "fcntl" | "fcntl64" => at!(fcntl),
            "mkstemp" | "mkstemp64" => at!(mkstemp),
            "close" => at!(close),
            "opendir" => at!(opendir),
            "fdopendir" => at!(fdopendir),
            "closedir" => at!(closedir),
            "mmap" | "mmap64" => at!(mmap),
            "munmap" => at!(munmap),
            "signal" => at!(signal),
            "sigaction" => at!(sigaction),
            "pthread_key_create" => at!(pthread_key_create),
            "pthread_key_delete" => at!(pthread_key_delete),
            "posix_spawn" => at!(posix_spawn),
            "posix_spawnp" => at!(posix_spawnp),
            "waitpid" => at!(waitpid),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "accept4" => at!(accept4),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "pipe2" => at!(pipe2),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "dup3" => at!(dup3),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "eventfd" => at!(eventfd),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "inotify_init1" => at!(inotify_init1),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "epoll_create1" => at!(epoll_create1),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "timerfd_create" => at!(timerfd_create),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "memfd_create" => at!(memfd_create),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "mremap" => at!(mremap),
            #[cfg(any(target_os = "linux", target_os = "android"))]
            "syscall" => at!(syscall),
            #[cfg(target_vendor = "apple")]
            "kqueue" => at!(kqueue),
            _ => None,
        }
    }

    /// The program's `pthread_atfork`, kept here (`ATFORK`): libc would call the handlers after the
    /// run's end, and on threads not in its domain.
    pub extern "C" fn pthread_atfork(prepare: ForkFn, parent: ForkFn, child: ForkFn) -> c_int {
        // A fork racing this registration must not take the registry as unused.
        ATFORK_USED.store(true, std::sync::atomic::Ordering::SeqCst);
        let domain = crate::domain::current();
        let _kept = crate::zone_alloc::kept();
        ATFORK.lock().unwrap_or_else(|e| e.into_inner()).push((domain, prepare, parent, child));
        0
    }

    /// The handlers of the forking thread's domain, for `fork_child`/`fork_parent` after the fork
    /// (no lock there). `prepare` runs now, last registered first.
    pub(crate) fn atfork_prepare() -> Vec<(ForkFn, ForkFn)> {
        let domain = crate::domain::current();
        let list: Vec<_> = {
            let _kept = crate::zone_alloc::kept();
            ATFORK.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|h| h.0 == domain).copied().collect()
        };
        for h in list.iter().rev() {
            if let Some(f) = h.1 {
                unsafe { f() };
            }
        }
        list.into_iter().map(|h| (h.2, h.3)).collect()
    }
    /// After the fork, in the parent, registration order.
    pub(crate) fn atfork_parent(list: &[(ForkFn, ForkFn)]) {
        for h in list {
            if let Some(f) = h.0 {
                unsafe { f() };
            }
        }
    }
    /// In any forked child, first thing: nothing here may wait for a lock any more.
    pub(crate) fn fork_child() {
        FORKED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    /// After the fork, in the child.
    pub(crate) fn atfork_child(list: &[(ForkFn, ForkFn)]) {
        for h in list {
            if let Some(f) = h.1 {
                unsafe { f() };
            }
        }
    }

    pub(super) fn give_back_callback(res: Res) -> bool {
        match res {
            // (a thread leaving the run finds no destructor to call)
            Res::Key(k) => {
                unsafe { sys::pthread_key_delete(k) };
                true
            }
            _ => false,
        }
    }

    pub(super) fn take_callbacks(bases: &[u64]) -> u32 {
        let mut g = ATFORK.lock().unwrap_or_else(|e| e.into_inner());
        let n = g.len();
        g.retain(|h| !bases.contains(&h.0));
        (n - g.len()) as u32
    }

    /// Gives one resource back, a child's end aside (`end_children`).
    pub(super) fn give_back(res: Res, out: &mut Released) {
        match res {
            Res::Fd { fd, dev, ino, socket } => {
                if identity(fd).is_some_and(|i| i.0 == dev && i.1 == ino) {
                    unsafe { sys::close(fd) };
                    if socket { out.sockets += 1 } else { out.files += 1 }
                }
            }
            Res::Dir(d) => {
                unsafe { sys::closedir(d as *mut c_void) };
                out.files += 1;
            }
            Res::Map { addr, len } => {
                unsafe { sys::munmap(addr as *mut u8, len) };
                out.mappings += 1;
                out.mapped_bytes += len as u64;
            }
            Res::Key(_) | Res::Child { .. } => {}
        }
    }

    const WNOHANG: c_int = 1;
    const SIGTERM: c_int = 15;
    const SIGKILL: c_int = 9;

    /// A signal to the child and the group it leads (what it started, in that case).
    fn signal_child(pid: i32, sig: c_int) {
        let target = if unsafe { sys::getpgid(pid) } == pid { -pid } else { pid };
        unsafe { sys::kill(target, sig) };
    }

    /// Ends the processes in `list`, and gives back what remains to be looked at again.
    pub(super) fn end_children(list: Vec<Res>, out: &mut Released) -> Vec<Res> {
        let own_session = unsafe { sys::getsid(0) };
        let mut left = Vec::new();
        let mut ending = Vec::new();
        for res in list {
            let Res::Child { pid, told, detached } = res else { continue };
            if unsafe { sys::waitpid(pid, std::ptr::null_mut(), WNOHANG) } != 0 {
                continue;
            }
            if detached {
                left.push(res);
            } else if unsafe { sys::getsid(pid) } != own_session {
                out.children_detached += 1;
                left.push(Res::Child { pid, told, detached: true });
            } else {
                signal_child(pid, if told { SIGKILL } else { SIGTERM });
                ending.push(pid);
            }
        }
        let wait = |ending: &mut Vec<i32>, out: &mut Released, grace: std::time::Duration| {
            let start = std::time::Instant::now();
            while !ending.is_empty() && start.elapsed() < grace {
                ending.retain(|pid| {
                    let gone = unsafe { sys::waitpid(*pid, std::ptr::null_mut(), WNOHANG) } != 0;
                    out.children_ended += gone as u32;
                    !gone
                });
                if !ending.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        };
        wait(&mut ending, out, CHILD_GRACE);
        for pid in &ending {
            signal_child(*pid, SIGKILL);
        }
        wait(&mut ending, out, CHILD_KILL_GRACE);
        out.children_left += ending.len() as u32;
        left.extend(ending.into_iter().map(|pid| Res::Child { pid, told: true, detached: false }));
        left
    }

    pub(super) fn is_child(res: &Res) -> bool {
        matches!(res, Res::Child { .. })
    }
}

#[cfg(windows)]
use windows::{Kind, Res};
#[cfg(windows)]
pub(crate) use windows::*;
#[cfg(windows)]
mod windows {
    use super::*;
    use crate::os::win32 as os;
    use crate::os::win32::{HANDLE, ProcessInformation, SecurityAttributes};

    const INVALID: HANDLE = -1;
    const WAIT_OBJECT_0: u32 = 0;
    const DUPLICATE_CLOSE_SOURCE: u32 = 1;
    const DUPLICATE_SAME_ACCESS: u32 = 2;
    const DETACHED_PROCESS: u32 = 0x8;
    const MEM_RESERVE: u32 = 0x2000;
    const MEM_RELEASE: u32 = 0x8000;

    #[derive(Clone, Copy)]
    pub(super) enum Res {
        Handle(HANDLE),
        /// The program's handle to a child this table records (`Child`): when it closes on a
        /// child that ended, that record goes too.
        ChildHandle { h: HANDLE, pid: u32 },
        Find(HANDLE),
        Socket(usize),
        Memory { addr: usize, len: usize },
        View { addr: usize, len: usize },
        /// The runtime's own handle to a child the run started, to end it with the run.
        Child { process: HANDLE, pid: u32, detached: bool },
    }

    /// The handle a record holds open, for `relax_held_handles` (a socket is one; a find handle
    /// is the system's context for the search, not the directory's handle).
    pub(super) fn held_handle(res: &Res) -> Option<u64> {
        match *res {
            Res::Handle(h) | Res::ChildHandle { h, .. } | Res::Child { process: h, .. } => Some(h as u64),
            Res::Socket(s) => Some(s as u64),
            Res::Find(_) | Res::Memory { .. } | Res::View { .. } => None,
        }
    }

    /// What a record's number is a number of (`Table::held`'s key with it). A socket is a kernel
    /// handle: it shares their numbers, so it shares their kind.
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub(super) enum Kind {
        Handle,
        Find,
        Memory,
        View,
        Child,
    }

    impl Kind {
        pub(super) fn name(self) -> &'static str {
            match self {
                Kind::Handle => "handle",
                Kind::Find => "find handle",
                Kind::Memory => "reservation",
                Kind::View => "view",
                Kind::Child => "child process",
            }
        }
    }

    pub(super) fn place(res: &Res) -> Place {
        match *res {
            Res::Handle(h) | Res::ChildHandle { h, .. } => Place::Key(Kind::Handle, h as u64),
            Res::Socket(s) => Place::Key(Kind::Handle, s as u64),
            Res::Find(h) => Place::Key(Kind::Find, h as u64),
            Res::Memory { addr, .. } => Place::Key(Kind::Memory, addr as u64),
            Res::View { addr, .. } => Place::Key(Kind::View, addr as u64),
            Res::Child { pid, .. } => Place::Child(pid as u64),
        }
    }

    /// What would call the run's code from threads not its own (`take_back_callbacks`): nothing
    /// in this table on Windows.
    pub(super) fn is_callback(_res: &Res) -> bool {
        false
    }

    /// A handle: NULL means it failed.
    fn note_handle(h: HANDLE) -> HANDLE {
        if h != 0 && h != INVALID {
            note(Res::Handle(h));
        }
        h
    }
    fn note_socket(s: usize) -> usize {
        if s != usize::MAX {
            note(Res::Socket(s));
        }
        s
    }
    fn forget(kind: Kind, n: u64) {
        with_table(|t| {
            t.remove(kind, n);
        });
    }

    extern "system" fn create_file(name: *const u16, access: u32, share: u32, sa: *mut u8, disposition: u32, flags: u32, template: HANDLE) -> HANDLE {
        note_handle(unsafe { os::CreateFileW(name, access, share, sa, disposition, flags, template) })
    }
    extern "system" fn create_file2(name: *const u16, access: u32, share: u32, disposition: u32, params: *const u8) -> HANDLE {
        note_handle(unsafe { os::CreateFile2(name, access, share, disposition, params) })
    }
    extern "system" fn reopen_file(h: HANDLE, access: u32, share: u32, flags: u32) -> HANDLE {
        note_handle(unsafe { os::ReOpenFile(h, access, share, flags) })
    }
    extern "system" fn open_file_by_id(volume: HANDLE, id: *const u8, access: u32, share: u32, security: *const u8, flags: u32) -> HANDLE {
        note_handle(unsafe { os::OpenFileById(volume, id, access, share, security, flags) })
    }
    // The native API's creates (crates that skip kernel32 for them: a poller's \Device\Afd, a
    // pipe made the way real std makes its child pipes): an NTSTATUS, the handle through `h`.
    fn note_nt(status: i32, h: *mut HANDLE) -> i32 {
        if status >= 0 {
            note_handle(unsafe { *h });
        }
        status
    }
    extern "system" fn nt_create_file(h: *mut HANDLE, access: u32, attrs: *const u8, io_status: *mut u8, size: *const i64, file_attrs: u32, share: u32, disposition: u32, options: u32, ea: *const u8, ea_len: u32) -> i32 {
        note_nt(unsafe { os::NtCreateFile(h, access, attrs, io_status, size, file_attrs, share, disposition, options, ea, ea_len) }, h)
    }
    extern "system" fn nt_open_file(h: *mut HANDLE, access: u32, attrs: *const u8, io_status: *mut u8, share: u32, options: u32) -> i32 {
        note_nt(unsafe { os::NtOpenFile(h, access, attrs, io_status, share, options) }, h)
    }
    extern "system" fn nt_create_named_pipe_file(h: *mut HANDLE, access: u32, attrs: *const u8, io_status: *mut u8, share: u32, disposition: u32, options: u32, pipe_type: u32, read_mode: u32, completion: u32, instances: u32, inbound: u32, outbound: u32, timeout: *const i64) -> i32 {
        note_nt(unsafe { os::NtCreateNamedPipeFile(h, access, attrs, io_status, share, disposition, options, pipe_type, read_mode, completion, instances, inbound, outbound, timeout) }, h)
    }
    extern "system" fn create_file_mapping(file: HANDLE, sa: *const SecurityAttributes, protect: u32, hi: u32, lo: u32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateFileMappingW(file, sa, protect, hi, lo, name) })
    }
    extern "system" fn create_event(sa: *const u8, manual: i32, initial: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateEventW(sa, manual, initial, name) })
    }
    extern "system" fn create_event_ex(sa: *const u8, name: *const u16, flags: u32, access: u32) -> HANDLE {
        note_handle(unsafe { os::CreateEventExW(sa, name, flags, access) })
    }
    extern "system" fn create_mutex(sa: *const u8, initial: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateMutexW(sa, initial, name) })
    }
    extern "system" fn create_semaphore(sa: *const u8, initial: i32, max: i32, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateSemaphoreW(sa, initial, max, name) })
    }
    extern "system" fn create_waitable_timer_ex(sa: *const u8, name: *const u16, flags: u32, access: u32) -> HANDLE {
        note_handle(unsafe { os::CreateWaitableTimerExW(sa, name, flags, access) })
    }
    extern "system" fn create_io_completion_port(file: HANDLE, existing: HANDLE, key: usize, threads: u32) -> HANDLE {
        let h = unsafe { os::CreateIoCompletionPort(file, existing, key, threads) };
        // (with a port already there, the same handle comes back)
        if existing == 0 { note_handle(h) } else { h }
    }
    extern "system" fn create_pipe(read: *mut HANDLE, write: *mut HANDLE, sa: *const u8, size: u32) -> i32 {
        let ok = unsafe { os::CreatePipe(read, write, sa, size) };
        if ok != 0 {
            unsafe {
                note_handle(*read);
                note_handle(*write);
            }
        }
        ok
    }
    extern "system" fn create_named_pipe(name: *const u16, mode: u32, pipe_mode: u32, instances: u32, out: u32, inp: u32, timeout: u32, sa: *const u8) -> HANDLE {
        note_handle(unsafe { os::CreateNamedPipeW(name, mode, pipe_mode, instances, out, inp, timeout, sa) })
    }
    extern "system" fn create_job(sa: *const u8, name: *const u16) -> HANDLE {
        note_handle(unsafe { os::CreateJobObjectW(sa, name) })
    }
    extern "system" fn open_process(access: u32, inherit: i32, pid: u32) -> HANDLE {
        let h = unsafe { os::OpenProcess(access, inherit, pid) };
        if h != 0 && with_table(|t| t.children.contains_key(&(pid as u64))) {
            note(Res::ChildHandle { h, pid });
            return h;
        }
        note_handle(h)
    }
    extern "system" fn open_thread(access: u32, inherit: i32, id: u32) -> HANDLE {
        note_handle(unsafe { os::OpenThread(access, inherit, id) })
    }
    extern "system" fn duplicate_handle(sp: HANDLE, s: HANDLE, tp: HANDLE, t: *mut HANDLE, access: u32, inherit: i32, options: u32) -> i32 {
        let me = unsafe { os::GetCurrentProcess() };
        // DUPLICATE_CLOSE_SOURCE closes the source, whether or not the copy is made: its record
        // goes first, as in close_handle (once it is closed, another thread can get its number,
        // and the run's end would close whatever has that number then).
        let close = sp == me && options & DUPLICATE_CLOSE_SOURCE != 0;
        // (a copy of a handle to a recorded child is one too)
        let child = if sp != me {
            None
        } else {
            with_table(|table| {
                let child = match table.held.get(&(Kind::Handle, s as u64))?.res {
                    Res::ChildHandle { pid, .. } => Some(pid),
                    _ => None,
                };
                if close {
                    table.remove(Kind::Handle, s as u64);
                }
                child
            })
        };
        let ok = unsafe { os::DuplicateHandle(sp, s, tp, t, access, inherit, options) };
        let mut copied = false;
        if ok != 0 && !t.is_null() && tp == me {
            let h = unsafe { *t };
            match child {
                Some(pid) if h != 0 => {
                    note(Res::ChildHandle { h, pid });
                    copied = true;
                }
                _ => {
                    note_handle(h);
                }
            }
        }
        // (the program's handle to a child closed and no copy of it here: as in close_handle)
        if let (true, Some(pid), false) = (close, child, copied) {
            forget_ended_children(Some(pid));
        }
        ok
    }
    /// A handle the program closes: its record goes first (once it is closed, another thread can
    /// get its number). The pid when it is a handle to a recorded child.
    fn closing(h: HANDLE) -> Option<u32> {
        with_table(|t| match t.remove(Kind::Handle, h as u64)?.res {
            Res::ChildHandle { pid, .. } => Some(pid),
            _ => None,
        })
    }
    /// A child that ended needs its record no longer: the program closing a handle to it is when
    /// to look (one wait, and only for such a handle), not its next spawn.
    fn closed(child: Option<u32>) {
        if let Some(pid) = child {
            forget_ended_children(Some(pid));
        }
    }
    extern "system" fn close_handle(h: HANDLE) -> i32 {
        let child = closing(h);
        let ok = unsafe { os::CloseHandle(h) };
        closed(child);
        ok
    }
    // (CloseHandle's own NtClose, inside the system's DLLs, is not the program's: only the
    // program's imports bind to these, so a close prunes its record once)
    extern "system" fn nt_close(h: HANDLE) -> i32 {
        let child = closing(h);
        let status = unsafe { os::NtClose(h) };
        closed(child);
        status
    }
    extern "system" fn create_process(app: *const u16, cmd: *mut u16, pa: *const u8, ta: *const u8, inherit: i32, flags: u32, env: *mut u8, cwd: *const u16, si: *mut u8, pi: *mut ProcessInformation) -> i32 {
        let ok = unsafe { os::CreateProcessW(app, cmd, pa, ta, inherit, flags, env, cwd, si, pi) };
        if ok != 0 {
            let pi = unsafe { &*pi };
            // (the run's own handle may be closed long before the process ends; with no run to
            // record the child, no copy: nothing would ever close it)
            let me = unsafe { os::GetCurrentProcess() };
            let mut process = 0;
            if owner().is_some() && unsafe { os::DuplicateHandle(me, pi.process, me, &mut process, 0, 0, DUPLICATE_SAME_ACCESS) } != 0 {
                forget_ended_children(None);
                note(Res::Child { process, pid: pi.pid, detached: flags & DETACHED_PROCESS != 0 });
                note(Res::ChildHandle { h: pi.process, pid: pi.pid });
            } else {
                note_handle(pi.process);
            }
            note_handle(pi.thread);
        }
        ok
    }
    /// Those that are over stop holding their process's record: every child, or the one `only`.
    /// (The looks at the processes are made outside the table's lock, and a record goes only if it
    /// is still the one looked at: a run's end may have taken it meanwhile, and closes it then.)
    fn forget_ended_children(only: Option<u32>) {
        let process_of = |o: &Owned| match o.res {
            Res::Child { process, pid, .. } => Some((pid, process)),
            _ => None,
        };
        let look: Vec<(u32, HANDLE)> = with_table(|t| match only {
            Some(pid) => t.children.get(&(pid as u64)).and_then(process_of).into_iter().collect(),
            None => t.children.values().filter_map(process_of).collect(),
        });
        let ended: Vec<(u32, HANDLE)> = look.into_iter().filter(|&(_, process)| unsafe { os::WaitForSingleObject(process, 0) } == WAIT_OBJECT_0).collect();
        if ended.is_empty() {
            return;
        }
        let gone: Vec<HANDLE> = with_table(|t| {
            let mut gone = Vec::new();
            for &(pid, process) in &ended {
                if t.children.get(&(pid as u64)).is_some_and(|o| matches!(o.res, Res::Child { process: p, .. } if p == process)) {
                    t.children.remove(&(pid as u64));
                    gone.push(process);
                }
            }
            gone
        });
        for process in gone {
            unsafe { os::CloseHandle(process) };
        }
    }
    extern "system" fn find_first_ex(name: *const u16, level: i32, data: *mut u8, op: i32, filter: *const u8, flags: u32) -> HANDLE {
        let h = unsafe { os::FindFirstFileExW(name, level, data, op, filter, flags) };
        if h != INVALID {
            note(Res::Find(h));
        }
        h
    }
    extern "system" fn find_first(name: *const u16, data: *mut u8) -> HANDLE {
        let h = unsafe { os::FindFirstFileW(name, data) };
        if h != INVALID {
            note(Res::Find(h));
        }
        h
    }
    extern "system" fn find_close(h: HANDLE) -> i32 {
        forget(Kind::Find, h as u64);
        unsafe { os::FindClose(h) }
    }
    extern "system" fn socket(af: i32, ty: i32, protocol: i32) -> usize {
        note_socket(unsafe { os::socket(af, ty, protocol) })
    }
    extern "system" fn wsa_socket(af: i32, ty: i32, protocol: i32, info: *const u8, group: u32, flags: u32) -> usize {
        note_socket(unsafe { os::WSASocketW(af, ty, protocol, info, group, flags) })
    }
    extern "system" fn accept(s: usize, addr: *mut u8, len: *mut i32) -> usize {
        note_socket(unsafe { os::accept(s, addr, len) })
    }
    extern "system" fn closesocket(s: usize) -> i32 {
        forget(Kind::Handle, s as u64);
        unsafe { os::closesocket(s) }
    }
    extern "system" fn virtual_alloc(addr: *mut u8, len: usize, kind: u32, protect: u32) -> *mut u8 {
        let p = unsafe { os::VirtualAlloc(addr, len, kind, protect) };
        // (a commit in what it reserved is not another one)
        if !p.is_null() && kind & MEM_RESERVE != 0 {
            note(Res::Memory { addr: p as usize, len });
        }
        p
    }
    extern "system" fn virtual_free(addr: *mut u8, len: usize, kind: u32) -> i32 {
        if kind & MEM_RELEASE != 0 {
            forget(Kind::Memory, addr as u64);
        }
        unsafe { os::VirtualFree(addr, len, kind) }
    }
    fn note_view(p: *mut u8, len: usize) -> *mut u8 {
        if !p.is_null() {
            note(Res::View { addr: p as usize, len });
        }
        p
    }
    extern "system" fn map_view(h: HANDLE, access: u32, hi: u32, lo: u32, size: usize) -> *mut u8 {
        note_view(unsafe { os::MapViewOfFile(h, access, hi, lo, size) }, size)
    }
    extern "system" fn map_view_ex(h: HANDLE, access: u32, hi: u32, lo: u32, size: usize, base: *mut u8) -> *mut u8 {
        note_view(unsafe { os::MapViewOfFileEx(h, access, hi, lo, size, base) }, size)
    }
    extern "system" fn unmap_view(addr: *mut u8) -> i32 {
        forget(Kind::View, addr as u64);
        unsafe { os::UnmapViewOfFile(addr) }
    }

    /// The run's address for a C symbol it binds (jit.rs `lookup_interposer`).
    pub(crate) fn interposer(link: &str) -> Option<u64> {
        macro_rules! at {
            ($f:expr) => { Some($f as *const () as usize as u64) };
        }
        match link {
            "CreateFileW" => at!(create_file),
            "CreateFile2" => at!(create_file2),
            "ReOpenFile" => at!(reopen_file),
            "OpenFileById" => at!(open_file_by_id),
            "NtCreateFile" => at!(nt_create_file),
            "NtOpenFile" => at!(nt_open_file),
            "NtCreateNamedPipeFile" => at!(nt_create_named_pipe_file),
            "NtClose" => at!(nt_close),
            "CreateFileMappingW" => at!(create_file_mapping),
            "CreateEventW" => at!(create_event),
            "CreateEventExW" => at!(create_event_ex),
            "CreateMutexW" => at!(create_mutex),
            "CreateSemaphoreW" => at!(create_semaphore),
            "CreateWaitableTimerExW" => at!(create_waitable_timer_ex),
            "CreateIoCompletionPort" => at!(create_io_completion_port),
            "CreatePipe" => at!(create_pipe),
            "CreateNamedPipeW" => at!(create_named_pipe),
            "CreateJobObjectW" => at!(create_job),
            "OpenProcess" => at!(open_process),
            "OpenThread" => at!(open_thread),
            "DuplicateHandle" => at!(duplicate_handle),
            "CloseHandle" => at!(close_handle),
            "CreateProcessW" => at!(create_process),
            "FindFirstFileExW" => at!(find_first_ex),
            "FindFirstFileW" => at!(find_first),
            "FindClose" => at!(find_close),
            "socket" => at!(socket),
            "WSASocketW" => at!(wsa_socket),
            "accept" => at!(accept),
            "closesocket" => at!(closesocket),
            "VirtualAlloc" => at!(virtual_alloc),
            "VirtualFree" => at!(virtual_free),
            "MapViewOfFile" => at!(map_view),
            "MapViewOfFileEx" => at!(map_view_ex),
            "UnmapViewOfFile" => at!(unmap_view),
            _ => None,
        }
    }

    pub(super) fn give_back_callback(_res: Res) -> bool {
        false
    }

    pub(super) fn take_callbacks(_bases: &[u64]) -> u32 {
        0
    }

    /// Gives one resource back, a child's end aside (`end_children`).
    pub(super) fn give_back(res: Res, out: &mut Released) {
        unsafe {
            match res {
                Res::Handle(h) | Res::ChildHandle { h, .. } => {
                    os::CloseHandle(h);
                    out.files += 1;
                }
                Res::Find(h) => {
                    os::FindClose(h);
                    out.files += 1;
                }
                Res::Socket(s) => {
                    os::closesocket(s);
                    out.sockets += 1;
                }
                Res::Memory { addr, len } => {
                    os::VirtualFree(addr as *mut u8, 0, MEM_RELEASE);
                    out.mappings += 1;
                    out.mapped_bytes += len as u64;
                }
                Res::View { addr, len } => {
                    os::UnmapViewOfFile(addr as *mut u8);
                    out.mappings += 1;
                    out.mapped_bytes += len as u64;
                }
                Res::Child { .. } => {}
            }
        }
    }

    /// Ends the processes in `list`, and gives back what remains to be looked at again. There is
    /// no asking here: a process is stopped. (Everything in a job the run made went when the
    /// job's handle closed.)
    pub(super) fn end_children(list: Vec<Res>, out: &mut Released) -> Vec<Res> {
        let mut ending = Vec::new();
        for res in list {
            let Res::Child { process, pid, detached } = res else { continue };
            if detached {
                unsafe { os::CloseHandle(process) };
                out.children_detached += 1;
            } else if unsafe { os::WaitForSingleObject(process, 0) } == WAIT_OBJECT_0 {
                unsafe { os::CloseHandle(process) };
            } else {
                unsafe { os::TerminateProcess(process, 1) };
                ending.push((process, pid));
            }
        }
        let start = std::time::Instant::now();
        while !ending.is_empty() && start.elapsed() < CHILD_GRACE + CHILD_KILL_GRACE {
            ending.retain(|&(h, _)| {
                let gone = unsafe { os::WaitForSingleObject(h, 0) } == WAIT_OBJECT_0;
                if gone {
                    unsafe { os::CloseHandle(h) };
                    out.children_ended += 1;
                }
                !gone
            });
            if !ending.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        out.children_left += ending.len() as u32;
        ending.into_iter().map(|(process, pid)| Res::Child { process, pid, detached: false }).collect()
    }

    pub(super) fn is_child(res: &Res) -> bool {
        matches!(res, Res::Child { .. })
    }
}

/// The domain of what outlived its run and is looked at again at the next end (a child: its exit
/// is reaped there). No run has this base.
const OUTLIVED: u64 = 0;

/// How long a run's children get to end by themselves, before they are made to.
const CHILD_GRACE: std::time::Duration = std::time::Duration::from_millis(500);
/// And how long the next run waits for those, before it starts anyway.
const CHILD_KILL_GRACE: std::time::Duration = std::time::Duration::from_millis(100);

/// Before a run's code ends: what would call it from threads not its own. Returns how many.
pub(crate) fn take_back_callbacks(bases: &[u64]) -> u32 {
    let mut n = take_callbacks(bases);
    #[cfg(unix)]
    {
        n += crate::os::progsig::give_back(bases);
    }
    crate::os::proc::forget_program_atexit(bases);
    let mut taken = with_table(|t| t.take(|o| bases.contains(&o.domain) && is_callback(&o.res)));
    // (last taken first)
    taken.sort_unstable_by(|a, b| b.seq.cmp(&a.seq));
    for o in taken {
        give_back_callback(o.res);
        n += 1;
    }
    n
}

/// After a run's threads have left: everything of its still open. Its children may take a moment
/// (`CHILD_GRACE`): nothing waits for this table meanwhile.
///
/// The order is the reverse of the order the run made them in, by design: nothing given back here
/// depends on another record still being there (a view keeps its section alive without the
/// section's handle; a child is ended through the runtime's own handle to it, `end_children`,
/// not the program's; on unix a descriptor, a directory and a mapping are each on their own).
pub(crate) fn give_back_run(bases: &[u64]) -> Released {
    let mut out = Released::default();
    let mut taken = with_table(|t| t.take(|o| bases.contains(&o.domain) || o.domain == OUTLIVED));
    taken.sort_unstable_by(|a, b| b.seq.cmp(&a.seq));
    let mut children = Vec::new();
    for o in taken {
        if is_child(&o.res) {
            children.push(o.res);
        } else {
            give_back(o.res, &mut out);
        }
    }
    let left = end_children(children, &mut out);
    with_table(|t| {
        for res in left {
            t.insert(OUTLIVED, res, false);
        }
    });
    out
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&OWNED, Mutex::new(Table::new()));
    reset(&REPLACED, [const { AtomicU64::new(0) }; 8]);
    reset(&REPLACED_SAID, AtomicU32::new(0));
    reset(&LEFT_OUT, AtomicU64::new(0));
    reset(&LEFT_OUT_SAID, AtomicU32::new(0));
    reset(&HANDLER_RECORDS, AtomicU64::new(0));
    reset(&RECORD_MAIN, std::sync::atomic::AtomicBool::new(false));
    #[cfg(unix)]
    reset(&ATFORK, Mutex::new(Vec::new()));
    #[cfg(unix)]
    reset(&ATFORK_USED, std::sync::atomic::AtomicBool::new(false));
}
