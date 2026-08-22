//! macOS/Apple-silicon storage + page-cache probe for model weight files.
//!
//! Sibling of `storage_bench.rs`, which targets Windows/NVMe: there the
//! question is "how do we reach queue depth", because a buffered
//! synchronous `ReadFile` loop leaves an NVMe mostly idle. On an Apple
//! Fabric SSD the physics are different, so this bin asks the questions
//! that actually matter here:
//!
//!   * what does one thread get, buffered vs uncached (`F_NOCACHE`)?
//!   * does threading help at all, or is one stream already the ceiling?
//!   * how does mmap + fault-in compare to read() into an arena, given
//!     that unified memory means the mapped pages ARE the GPU's pages?
//!   * how warm is warm — what does a page-cache-hot re-read cost?
//!
//! The instrument that makes this honest is `mincore(2)`: for any mapped
//! file it reports, page by page, whether that page is resident in the
//! unified page cache. So "cold" and "warm" are measured, not assumed, and
//! an eviction can be verified instead of hoped for. (The CUDA-side lane
//! had to poll nvidia-smi for the equivalent question.)
//!
//! Nothing here executes a model: it only reads bytes and touches pages.
//!
//! usage: mac-storage-bench <file> [method ...] [flags]
//!   methods: cached:<MB>, nocache:<MB>, cached-threads:<n>:<MB>,
//!            nocache-threads:<n>:<MB>, pread-threads:<n>:<MB>,
//!            rdadvise:<MB>, mmap-fault, mmap-fault-threads:<n>,
//!            mmap-willneed, mmap-seq, residency, all
//!   flags:   --ring (default; reuse a small destination = pure device
//!            bandwidth) | --arena (full-size destination, first-touch
//!            page-fault cost included), --limit:<GB>, --repeat:<n>,
//!            --evict (drop the file from the page cache before each run),
//!            --verify (mincore residency before/after each run)

use std::path::Path;
use std::time::Instant;

mod sys {
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    pub const O_RDONLY: c_int = 0;
    pub const F_NOCACHE: c_int = 48;
    pub const F_RDADVISE: c_int = 44;
    pub const PROT_READ: c_int = 1;
    pub const MAP_SHARED: c_int = 1;
    pub const MADV_WILLNEED: c_int = 3;
    pub const MADV_SEQUENTIAL: c_int = 2;
    pub const MADV_DONTNEED: c_int = 4;
    pub const MS_INVALIDATE: c_int = 0x0002;
    pub const MS_KILLPAGES: c_int = 0x0004;

    #[repr(C)]
    pub struct Radvisory {
        pub ra_offset: i64,
        pub ra_count: c_int,
    }

    extern "C" {
        pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        pub fn close(fd: c_int) -> c_int;
        pub fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
        pub fn pread(fd: c_int, buf: *mut c_void, count: usize, offset: i64) -> isize;
        pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: i64,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
        pub fn madvise(addr: *mut c_void, len: usize, advice: c_int) -> c_int;
        pub fn msync(addr: *mut c_void, len: usize, flags: c_int) -> c_int;
        pub fn mincore(addr: *const c_void, len: usize, vec: *mut c_char) -> c_int;
        pub fn getpagesize() -> c_int;
    }

    pub fn page_size() -> usize {
        unsafe { getpagesize() as usize }
    }

    pub fn open_read(path: &Path, nocache: bool) -> Result<c_int, String> {
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "path contains NUL".to_string())?;
        let fd = unsafe { open(c_path.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return Err(format!("open {:?} failed", path));
        }
        if nocache && unsafe { fcntl(fd, F_NOCACHE, 1) } < 0 {
            unsafe { close(fd) };
            return Err("fcntl(F_NOCACHE) failed".to_string());
        }
        Ok(fd)
    }

    /// A whole-file read-only mapping, used both as a benchmark subject and
    /// as the handle through which residency is inspected.
    pub struct Mapping {
        pub ptr: *mut c_void,
        pub len: usize,
    }

    unsafe impl Send for Mapping {}
    unsafe impl Sync for Mapping {}

    impl Mapping {
        pub fn open(path: &Path, len: usize) -> Result<Self, String> {
            let fd = open_read(path, false)?;
            let page = page_size();
            let len = len.next_multiple_of(page);
            let ptr = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, fd, 0) };
            unsafe { close(fd) };
            if ptr.is_null() || ptr as isize == -1 {
                return Err("mmap failed".to_string());
            }
            Ok(Self { ptr, len })
        }

        /// Resident page count / total page count, straight from the kernel.
        pub fn residency(&self) -> (usize, usize) {
            let page = page_size();
            let pages = self.len / page;
            let mut vec = vec![0i8; pages];
            let rc = unsafe {
                mincore(
                    self.ptr as *const c_void,
                    self.len,
                    vec.as_mut_ptr() as *mut c_char,
                )
            };
            if rc != 0 {
                return (0, pages);
            }
            (vec.iter().filter(|b| **b & 1 != 0).count(), pages)
        }
    }

    impl Drop for Mapping {
        fn drop(&mut self) {
            unsafe {
                munmap(self.ptr, self.len);
            }
        }
    }
}

/// Destination for read() based methods. `Ring` reuses one small buffer so
/// the number is pure device bandwidth; `Arena` writes into a full-size
/// allocation so the first-touch page-fault cost is included, which is what
/// a real weight load pays.
enum Dest {
    Ring(Vec<u8>),
    Arena(Vec<u8>),
}

impl Dest {
    fn new(arena: bool, total: u64, chunk: usize) -> Self {
        if arena {
            Dest::Arena(vec![0u8; total as usize])
        } else {
            Dest::Ring(vec![0u8; chunk])
        }
    }

    /// Slice to receive `want` bytes for the read that starts at `at`.
    fn slot(&mut self, at: u64, want: usize) -> &mut [u8] {
        match self {
            Dest::Ring(buf) => {
                let end = want.min(buf.len());
                &mut buf[..end]
            }
            Dest::Arena(buf) => {
                let start = at as usize;
                &mut buf[start..start + want]
            }
        }
    }
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!(
            "usage: mac-storage-bench <file> [method ...] [--ring|--arena] [--limit:<GB>] \
             [--repeat:<n>] [--evict] [--verify]"
        );
        std::process::exit(2);
    }
    let path = args[0].clone();
    let mut methods = Vec::new();
    let mut arena = false;
    let mut limit_bytes = u64::MAX;
    let mut repeat = 1usize;
    let mut evict = false;
    let mut verify = false;
    for arg in &args[1..] {
        if let Some(rest) = arg.strip_prefix("--limit:") {
            limit_bytes = (rest.parse::<f64>().unwrap_or(0.0) * (1u64 << 30) as f64) as u64;
        } else if let Some(rest) = arg.strip_prefix("--repeat:") {
            repeat = rest.parse().unwrap_or(1);
        } else if arg == "--arena" {
            arena = true;
        } else if arg == "--ring" {
            arena = false;
        } else if arg == "--evict" {
            evict = true;
        } else if arg == "--verify" {
            verify = true;
        } else {
            methods.push(arg.clone());
        }
    }
    if methods.is_empty() || methods.iter().any(|m| m == "all") {
        methods = default_methods();
    }

    let file_size = match std::fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            eprintln!("mac-storage-bench: cannot stat {}: {}", path, err);
            std::process::exit(1);
        }
    };
    let bytes = file_size.min(limit_bytes);
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;
    println!(
        "file: {} ({:.2} GB, benching {:.2} GB, dest={}, page={} KB)",
        path,
        gb(file_size),
        gb(bytes),
        if arena { "arena" } else { "ring" },
        sys::page_size() / 1024
    );
    if let Ok(map) = sys::Mapping::open(Path::new(&path), bytes as usize) {
        let (resident, pages) = map.residency();
        println!(
            "start residency: {:.1}% ({} / {} pages resident in the page cache)",
            100.0 * resident as f64 / pages.max(1) as f64,
            resident,
            pages
        );
    }

    for method in &methods {
        for _ in 0..repeat {
            if evict {
                match evict_file(Path::new(&path), bytes) {
                    Ok((before, after, pages)) => {
                        if verify {
                            println!(
                                "  evict: {} -> {} of {} pages resident",
                                before, after, pages
                            );
                        }
                        if after * 20 > pages {
                            println!(
                                "  WARNING: evict left {:.0}% resident; this run is not cold",
                                100.0 * after as f64 / pages.max(1) as f64
                            );
                        }
                    }
                    Err(err) => println!("  evict failed: {}", err),
                }
            }
            let started = Instant::now();
            let result = run_method(method, Path::new(&path), bytes, arena);
            let elapsed = started.elapsed().as_secs_f64();
            match result {
                Ok(read) => {
                    let mut line = format!(
                        "{:<26} {:>8.3} s  {:>7.2} GB/s  ({:.2} GB)",
                        method,
                        elapsed,
                        gb(read) / elapsed,
                        gb(read)
                    );
                    if verify {
                        if let Ok(map) = sys::Mapping::open(Path::new(&path), bytes as usize) {
                            let (resident, pages) = map.residency();
                            line.push_str(&format!(
                                "  resident_after={:.1}%",
                                100.0 * resident as f64 / pages.max(1) as f64
                            ));
                        }
                    }
                    println!("{}", line);
                }
                Err(err) => println!("{:<26} SKIP/ERR: {}", method, err),
            }
        }
    }
}

fn default_methods() -> Vec<String> {
    [
        "cached:1",
        "cached:4",
        "cached:16",
        "nocache:1",
        "nocache:4",
        "nocache:16",
        "nocache-threads:2:4",
        "nocache-threads:4:4",
        "nocache-threads:8:4",
        "cached-threads:4:4",
        "pread-threads:4:4",
        "rdadvise:4",
        "mmap-fault",
        "mmap-seq",
        "mmap-willneed",
        "mmap-fault-threads:4",
        "mmap-fault-threads:8",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn mb(n: usize) -> usize {
    n << 20
}

fn run_method(method: &str, path: &Path, bytes: u64, arena: bool) -> Result<u64, String> {
    let mut parts = method.split(':');
    let name = parts.next().unwrap_or("");
    let nums: Vec<usize> = parts.filter_map(|p| p.parse().ok()).collect();
    match name {
        "cached" => read_loop(path, bytes, mb(nums.first().copied().unwrap_or(4)), false, arena),
        "nocache" => read_loop(path, bytes, mb(nums.first().copied().unwrap_or(4)), true, arena),
        "cached-threads" => threaded(
            path,
            bytes,
            nums.first().copied().unwrap_or(4),
            mb(nums.get(1).copied().unwrap_or(4)),
            false,
            arena,
        ),
        "nocache-threads" => threaded(
            path,
            bytes,
            nums.first().copied().unwrap_or(4),
            mb(nums.get(1).copied().unwrap_or(4)),
            true,
            arena,
        ),
        // Same shape as `bulk_read`'s unix path: one fd per thread, pread,
        // buffered (it never sets F_NOCACHE).
        "pread-threads" => threaded(
            path,
            bytes,
            nums.first().copied().unwrap_or(4),
            mb(nums.get(1).copied().unwrap_or(4)),
            false,
            arena,
        ),
        "rdadvise" => rdadvise(path, bytes, mb(nums.first().copied().unwrap_or(4)), arena),
        "mmap-fault" => mmap_fault(path, bytes, 1, None),
        "mmap-seq" => mmap_fault(path, bytes, 1, Some(sys::MADV_SEQUENTIAL)),
        "mmap-willneed" => mmap_fault(path, bytes, 1, Some(sys::MADV_WILLNEED)),
        "mmap-fault-threads" => mmap_fault(path, bytes, nums.first().copied().unwrap_or(4), None),
        "mmap-willneed-threads" => mmap_fault(
            path,
            bytes,
            nums.first().copied().unwrap_or(4),
            Some(sys::MADV_WILLNEED),
        ),
        "rdadvise-mmap" => rdadvise_mmap(path, bytes, mb(nums.first().copied().unwrap_or(16))),
        "read-prime-mmap" => read_prime_mmap(path, bytes, mb(nums.first().copied().unwrap_or(4))),
        "mmap-willneed-win" => mmap_willneed_win(
            path,
            bytes,
            mb(nums.first().copied().unwrap_or(64)),
        ),
        "residency" => {
            let map = sys::Mapping::open(path, bytes as usize)?;
            let (resident, pages) = map.residency();
            println!(
                "  residency: {} / {} pages ({:.1}%)",
                resident,
                pages,
                100.0 * resident as f64 / pages.max(1) as f64
            );
            Ok(0)
        }
        other => Err(format!("unknown method {}", other)),
    }
}

/// Sequential `read()` loop on one descriptor.
fn read_loop(
    path: &Path,
    bytes: u64,
    chunk: usize,
    nocache: bool,
    arena: bool,
) -> Result<u64, String> {
    let fd = sys::open_read(path, nocache)?;
    let mut dest = Dest::new(arena, bytes, chunk);
    let mut done = 0u64;
    while done < bytes {
        let want = chunk.min((bytes - done) as usize);
        let slot = dest.slot(done, want);
        let want = slot.len();
        let got = unsafe { sys::read(fd, slot.as_mut_ptr() as *mut _, want) };
        if got <= 0 {
            break;
        }
        done += got as u64;
    }
    unsafe { sys::close(fd) };
    std::hint::black_box(&dest);
    Ok(done)
}

/// `F_RDADVISE` issues an async advisory read with no copy to user space:
/// the hint runs ahead of the synchronous read loop that follows it.
fn rdadvise(path: &Path, bytes: u64, chunk: usize, arena: bool) -> Result<u64, String> {
    let fd = sys::open_read(path, false)?;
    let mut dest = Dest::new(arena, bytes, chunk);
    let ahead = (chunk * 4) as u64;
    let mut done = 0u64;
    let mut hinted = 0u64;
    while done < bytes {
        while hinted < bytes && hinted < done + ahead {
            let count = chunk.min((bytes - hinted) as usize);
            let ra = sys::Radvisory {
                ra_offset: hinted as i64,
                ra_count: count as i32,
            };
            unsafe { sys::fcntl(fd, sys::F_RDADVISE, &ra as *const sys::Radvisory) };
            hinted += count as u64;
        }
        let want = chunk.min((bytes - done) as usize);
        let slot = dest.slot(done, want);
        let want = slot.len();
        let got = unsafe { sys::read(fd, slot.as_mut_ptr() as *mut _, want) };
        if got <= 0 {
            break;
        }
        done += got as u64;
    }
    unsafe { sys::close(fd) };
    std::hint::black_box(&dest);
    Ok(done)
}

/// Raw arena pointer handed to reader threads; each thread writes only its
/// own disjoint byte range.
#[derive(Clone, Copy)]
struct ArenaPtr(*mut u8);
unsafe impl Send for ArenaPtr {}
unsafe impl Sync for ArenaPtr {}

impl ArenaPtr {
    /// Taking `self` by value matters: a closure that named `base.0`
    /// directly would capture the bare `*mut u8` (edition-2021 disjoint
    /// capture) and lose the `Send` this wrapper exists to provide.
    fn at(self, offset: usize) -> *mut u8 {
        // Safety: callers only ever pass offsets inside their own disjoint
        // range of the destination allocation.
        unsafe { self.0.add(offset) }
    }
}

/// `n` threads, one descriptor each, `pread` of contiguous file segments.
/// This is the shape `bulk_read::read_placed` takes on unix.
fn threaded(
    path: &Path,
    bytes: u64,
    threads: usize,
    chunk: usize,
    nocache: bool,
    arena: bool,
) -> Result<u64, String> {
    let threads = threads.max(1);
    let mut storage: Vec<u8> = if arena {
        vec![0u8; bytes as usize]
    } else {
        vec![0u8; chunk * threads]
    };
    let base = ArenaPtr(storage.as_mut_ptr());
    let span = bytes.div_ceil(threads as u64);
    let mut handles = Vec::with_capacity(threads);
    for t in 0..threads {
        let start = span * t as u64;
        if start >= bytes {
            break;
        }
        let end = (start + span).min(bytes);
        let path = path.to_path_buf();
        let base = base;
        handles.push(std::thread::spawn(move || -> Result<u64, String> {
            let fd = sys::open_read(&path, nocache)?;
            let mut done = start;
            let mut total = 0u64;
            while done < end {
                let want = chunk.min((end - done) as usize);
                // Ring destinations give each thread its own slot so the
                // threads never contend for the same cache lines.
                let dst = if arena {
                    base.at(done as usize)
                } else {
                    base.at(t * chunk)
                };
                let got = unsafe { sys::pread(fd, dst as *mut _, want, done as i64) };
                if got <= 0 {
                    break;
                }
                done += got as u64;
                total += got as u64;
            }
            unsafe { sys::close(fd) };
            Ok(total)
        }));
    }
    let mut total = 0u64;
    for handle in handles {
        total += handle.join().map_err(|_| "reader thread panicked")??;
    }
    std::hint::black_box(&storage);
    Ok(total)
}

/// mmap the file and fault every page in by touching it. On unified memory
/// this is the shape that matters: the faulted pages are the same physical
/// pages the GPU will read through a no-copy buffer, so there is no second
/// copy anywhere in the chain.
fn mmap_fault(path: &Path, bytes: u64, threads: usize, advice: Option<i32>) -> Result<u64, String> {
    let map = sys::Mapping::open(path, bytes as usize)?;
    let page = sys::page_size();
    if let Some(advice) = advice {
        unsafe { sys::madvise(map.ptr, map.len, advice) };
    }
    let threads = threads.max(1);
    let len = (bytes as usize).min(map.len);
    let base = ArenaPtr(map.ptr as *mut u8);
    let span = len.div_ceil(threads);
    let mut handles = Vec::with_capacity(threads);
    for t in 0..threads {
        let start = span * t;
        if start >= len {
            break;
        }
        let end = (start + span).min(len);
        let base = base;
        handles.push(std::thread::spawn(move || -> u64 {
            let mut acc = 0u64;
            let mut off = start;
            while off < end {
                // One touch per page is all a fault needs.
                acc += unsafe { std::ptr::read_volatile(base.at(off)) } as u64;
                off += page;
            }
            std::hint::black_box(acc);
            (end - start) as u64
        }));
    }
    let mut total = 0u64;
    for handle in handles {
        total += handle.join().map_err(|_| "fault thread panicked")?;
    }
    Ok(total)
}

/// Candidate lever 1: keep the mmap (so Metal still gets a no-copy buffer
/// over file-backed pages) but stop paying for demand paging. A lookahead
/// thread issues `F_RDADVISE` — async advisory reads with no copy to user
/// space — so the pages are already in the unified page cache by the time
/// the fault cursor reaches them, turning hard faults into soft ones.
fn rdadvise_mmap(path: &Path, bytes: u64, window: usize) -> Result<u64, String> {
    let map = sys::Mapping::open(path, bytes as usize)?;
    let page = sys::page_size();
    let fd = sys::open_read(path, false)?;
    // Stay this far ahead of the faulting cursor: far enough to keep the
    // device busy, bounded so the hints cannot queue the whole file.
    const AHEAD: u64 = 256 << 20;
    let cursor = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hinter = {
        let cursor = cursor.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let mut hinted = 0u64;
            while hinted < bytes && !done.load(std::sync::atomic::Ordering::Relaxed) {
                let at = cursor.load(std::sync::atomic::Ordering::Relaxed);
                if hinted >= at + AHEAD {
                    std::thread::yield_now();
                    continue;
                }
                let count = (window as u64).min(bytes - hinted) as i32;
                let ra = sys::Radvisory {
                    ra_offset: hinted as i64,
                    ra_count: count,
                };
                unsafe { sys::fcntl(fd, sys::F_RDADVISE, &ra as *const sys::Radvisory) };
                hinted += count as u64;
            }
            unsafe { sys::close(fd) };
        })
    };
    let base = ArenaPtr(map.ptr as *mut u8);
    let len = (bytes as usize).min(map.len);
    let mut acc = 0u64;
    let mut off = 0usize;
    while off < len {
        acc += unsafe { std::ptr::read_volatile(base.at(off)) } as u64;
        off += page;
        if off % (16 << 20) == 0 {
            cursor.store(off as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    cursor.store(bytes, std::sync::atomic::Ordering::Relaxed);
    let _ = hinter.join();
    std::hint::black_box(acc);
    Ok(len as u64)
}

/// Candidate lever 2: populate the page cache with the fast path — a plain
/// buffered sequential `read()` into a small reused buffer — and only then
/// mmap and fault. The read bytes are thrown away; the point is the cache
/// they leave behind, which turns every later fault into a soft one.
fn read_prime_mmap(path: &Path, bytes: u64, chunk: usize) -> Result<u64, String> {
    let started = Instant::now();
    let primed = read_loop(path, bytes, chunk, false, false)?;
    let prime_s = started.elapsed().as_secs_f64();
    let fault_started = Instant::now();
    let faulted = mmap_fault(path, bytes, 1, None)?;
    println!(
        "    prime {:.3} s ({:.2} GB/s) + fault {:.3} s ({:.2} GB/s)",
        prime_s,
        primed as f64 / (1u64 << 30) as f64 / prime_s,
        fault_started.elapsed().as_secs_f64(),
        faulted as f64 / (1u64 << 30) as f64 / fault_started.elapsed().as_secs_f64()
    );
    Ok(faulted)
}

/// Candidate lever 3: `madvise(WILLNEED)` a window at a time from a
/// lookahead thread, rather than once over the whole mapping.
fn mmap_willneed_win(path: &Path, bytes: u64, window: usize) -> Result<u64, String> {
    let map = sys::Mapping::open(path, bytes as usize)?;
    let page = sys::page_size();
    let len = (bytes as usize).min(map.len);
    let base = ArenaPtr(map.ptr as *mut u8);
    let cursor = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let advisor = {
        let cursor = cursor.clone();
        let done = done.clone();
        let ptr = ArenaPtr(map.ptr as *mut u8);
        std::thread::spawn(move || {
            let mut hinted = 0usize;
            while hinted < len && !done.load(std::sync::atomic::Ordering::Relaxed) {
                let at = cursor.load(std::sync::atomic::Ordering::Relaxed) as usize;
                if hinted >= at + 2 * window {
                    std::thread::yield_now();
                    continue;
                }
                let count = window.min(len - hinted);
                unsafe {
                    sys::madvise(ptr.at(hinted) as *mut _, count, sys::MADV_WILLNEED);
                }
                hinted += count;
            }
        })
    };
    let mut acc = 0u64;
    let mut off = 0usize;
    while off < len {
        acc += unsafe { std::ptr::read_volatile(base.at(off)) } as u64;
        off += page;
        if off % (16 << 20) == 0 {
            cursor.store(off as u64, std::sync::atomic::Ordering::Relaxed);
        }
    }
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = advisor.join();
    std::hint::black_box(acc);
    Ok(len as u64)
}

/// Drop a single file's pages from the unified page cache, and prove it
/// with `mincore`. Nothing here touches any other file's cache — this is
/// per-file, not a system-wide `purge`.
///
/// `msync(MS_INVALIDATE)` is the primitive that actually works, and it
/// only works *alone*: measured on macOS 26.5, `MS_KILLPAGES`,
/// `MS_DEACTIVATE`, `MADV_DONTNEED` and `F_GLOBAL_NOCACHE` all return 0
/// and leave every page resident, and OR-ing `MS_KILLPAGES` into the
/// `MS_INVALIDATE` call silently defeats it. All of them "succeed", so
/// only the `mincore` check distinguishes a real eviction from a no-op —
/// which is why the caller verifies instead of trusting the return code.
fn evict_file(path: &Path, bytes: u64) -> Result<(usize, usize, usize), String> {
    let map = sys::Mapping::open(path, bytes as usize)?;
    let (before, pages) = map.residency();
    if unsafe { sys::msync(map.ptr, map.len, sys::MS_INVALIDATE) } != 0 {
        return Err("msync(MS_INVALIDATE) failed".to_string());
    }
    let (after, _) = map.residency();
    Ok((before, after, pages))
}
