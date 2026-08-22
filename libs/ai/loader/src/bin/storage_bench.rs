//! Storage read-bandwidth probe for model weight files.
//!
//! Measures how fast we can get a multi-GB weight file off the SSD and into
//! host memory, comparing the shapes the loader actually uses (whole-file
//! buffered read, per-tensor open+seek+read) against the shapes an NVMe
//! actually wants (unbuffered reads at queue depth, parallel segments).
//!
//! NVMe drives only reach rated bandwidth at queue depth: a single
//! synchronous buffered `ReadFile` loop leaves a large fraction of the
//! device idle between requests. The point of this probe is to put a number
//! on that gap on real hardware instead of guessing.
//!
//! Windows is the target (the CUDA boxes); the portable methods also run on
//! unix so the bin stays buildable everywhere.
//!
//! usage: storage-bench <file> [method ...]
//!   methods: std-whole, std-chunk:<MB>, per-tensor, per-tensor-1handle,
//!            nobuf:<MB>, nobuf-ov:<MB>:<depth>, seqscan:<MB>,
//!            threads:<n>:<MB>, nobuf-threads:<n>:<MB>, winmmap, all
//!   flags:   --arena (write into a full-size destination, first-touch cost
//!            included) | --ring (reuse a small destination; pure device
//!            bandwidth), --limit:<GB>, --flush:<other-file>, --repeat:<n>

use std::path::Path;
use std::time::Instant;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("usage: storage-bench <file> [method ...] [--arena|--ring] [--limit:<GB>] [--flush:<file>] [--repeat:<n>]");
        std::process::exit(2);
    }
    let path = args[0].clone();
    let mut methods = Vec::new();
    let mut arena = false;
    let mut limit_bytes = u64::MAX;
    let mut flush_with: Option<String> = None;
    let mut repeat = 1usize;
    for arg in &args[1..] {
        if let Some(rest) = arg.strip_prefix("--limit:") {
            limit_bytes = (rest.parse::<f64>().unwrap_or(0.0) * (1u64 << 30) as f64) as u64;
        } else if let Some(rest) = arg.strip_prefix("--flush:") {
            flush_with = Some(rest.to_string());
        } else if let Some(rest) = arg.strip_prefix("--repeat:") {
            repeat = rest.parse().unwrap_or(1);
        } else if arg == "--arena" {
            arena = true;
        } else if arg == "--ring" {
            arena = false;
        } else {
            methods.push(arg.clone());
        }
    }
    if methods.is_empty() {
        methods.push("all".to_string());
    }
    if methods.iter().any(|m| m == "all") {
        methods = default_methods();
    }

    let file_size = match std::fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            eprintln!("storage-bench: cannot stat {}: {}", path, err);
            std::process::exit(1);
        }
    };
    let bytes = file_size.min(limit_bytes);
    println!(
        "file: {} ({:.2} GB, benching {:.2} GB, dest={})",
        path,
        file_size as f64 / (1u64 << 30) as f64,
        bytes as f64 / (1u64 << 30) as f64,
        if arena { "arena" } else { "ring" }
    );

    for method in &methods {
        for iteration in 0..repeat {
            if let Some(flush) = &flush_with {
                let flushed = flush_page_cache(flush);
                if iteration == 0 && flushed > 0 {
                    println!(
                        "  (cache flush: streamed {:.1} GB of {})",
                        flushed as f64 / (1u64 << 30) as f64,
                        flush
                    );
                }
            }
            let started = Instant::now();
            let result = run_method(method, Path::new(&path), bytes, arena);
            let elapsed = started.elapsed().as_secs_f64();
            match result {
                Ok(read) => {
                    let gb = read as f64 / (1u64 << 30) as f64;
                    println!(
                        "{:<24} {:>8.3} s  {:>7.2} GB/s  ({:.2} GB)",
                        method,
                        elapsed,
                        gb / elapsed,
                        gb
                    );
                }
                Err(err) => println!("{:<24} SKIP/ERR: {}", method, err),
            }
        }
    }
}

fn default_methods() -> Vec<String> {
    [
        "std-whole",
        "std-chunk:4",
        "per-tensor",
        "per-tensor-1handle",
        "seqscan:4",
        "nobuf:1",
        "nobuf:4",
        "nobuf:16",
        "nobuf-ov:1:8",
        "nobuf-ov:1:32",
        "nobuf-ov:2:16",
        "nobuf-ov:4:8",
        "nobuf-ov:4:16",
        "nobuf-ov:4:32",
        "nobuf-ov:8:16",
        "threads:4:4",
        "threads:8:4",
        "nobuf-threads:2:4",
        "nobuf-threads:4:4",
        "nobuf-threads:8:4",
        "winmmap",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn parse_parts(method: &str) -> Vec<u64> {
    method
        .split(':')
        .skip(1)
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn run_method(method: &str, path: &Path, bytes: u64, arena: bool) -> Result<u64, String> {
    let parts = parse_parts(method);
    let name = method.split(':').next().unwrap_or(method);
    match name {
        "std-whole" => std_whole(path, bytes),
        "std-chunk" => std_chunk(path, bytes, mb(parts.first().copied().unwrap_or(4))),
        "per-tensor" => per_tensor(path, bytes, false, arena),
        "per-tensor-1handle" => per_tensor(path, bytes, true, arena),
        "seqscan" => platform::seq_scan(path, bytes, mb(parts.first().copied().unwrap_or(4)), arena),
        "nobuf" => platform::nobuf(path, bytes, mb(parts.first().copied().unwrap_or(4)), arena),
        "nobuf-ov" => platform::nobuf_overlapped(
            path,
            bytes,
            mb(parts.first().copied().unwrap_or(4)),
            parts.get(1).copied().unwrap_or(8) as usize,
            arena,
        ),
        "threads" => platform::threaded(
            path,
            bytes,
            parts.first().copied().unwrap_or(4) as usize,
            mb(parts.get(1).copied().unwrap_or(4)),
            false,
            arena,
        ),
        "nobuf-threads" => platform::threaded(
            path,
            bytes,
            parts.first().copied().unwrap_or(4) as usize,
            mb(parts.get(1).copied().unwrap_or(4)),
            true,
            arena,
        ),
        "prefault" => platform::prefault(bytes, parts.first().copied().unwrap_or(1) as usize),
        "scatter" => scatter(
            path,
            bytes,
            parts.first().copied().unwrap_or(8) as usize,
            mb(parts.get(1).copied().unwrap_or(4)),
        ),
        "winmmap" => platform::win_mmap(path, bytes),
        other => Err(format!("unknown method '{}'", other)),
    }
}

fn mb(n: u64) -> usize {
    (n as usize) << 20
}

/// Baseline A: what a "read the whole file" loader does.
fn std_whole(path: &Path, bytes: u64) -> Result<u64, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut dst = vec![0u8; bytes as usize];
    file.read_exact(&mut dst).map_err(|e| e.to_string())?;
    std::hint::black_box(&dst[0]);
    Ok(bytes)
}

/// Baseline B: chunked read into a reused buffer (no giant allocation).
fn std_chunk(path: &Path, bytes: u64, chunk: usize) -> Result<u64, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; chunk];
    let mut done = 0u64;
    while done < bytes {
        let want = chunk.min((bytes - done) as usize);
        file.read_exact(&mut buf[..want]).map_err(|e| e.to_string())?;
        std::hint::black_box(&buf[0]);
        done += want as u64;
    }
    Ok(done)
}

/// The shape the gguf loader uses today: for every tensor, open the file,
/// seek to its offset, read its bytes. Reads the real tensor table so the
/// request-size distribution matches a real load.
fn per_tensor(path: &Path, bytes: u64, single_handle: bool, arena: bool) -> Result<u64, String> {
    use std::io::{Read, Seek, SeekFrom};
    let segments = tensor_segments(path)?;
    let mut arena_buf = if arena { vec![0u8; bytes as usize] } else { Vec::new() };
    let mut scratch = Vec::new();
    let mut handle = if single_handle {
        Some(std::fs::File::open(path).map_err(|e| e.to_string())?)
    } else {
        None
    };
    let mut done = 0u64;
    for (offset, len) in segments {
        if done + len > bytes {
            break;
        }
        let dst: &mut [u8] = if arena {
            let start = done as usize;
            &mut arena_buf[start..start + len as usize]
        } else {
            if scratch.len() < len as usize {
                scratch.resize(len as usize, 0);
            }
            &mut scratch[..len as usize]
        };
        match handle.as_mut() {
            Some(file) => {
                file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
                file.read_exact(dst).map_err(|e| e.to_string())?;
            }
            None => {
                let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
                file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
                file.read_exact(dst).map_err(|e| e.to_string())?;
            }
        }
        done += len;
    }
    Ok(done)
}

/// The design under evaluation for the loader: split the tensor list across
/// N threads, each streaming its slice of the file with large unbuffered
/// reads into a staging block, then scatter-copying tensor bytes to their
/// arena destinations. Unbuffered I/O needs sector-aligned file offsets and
/// buffers, which per-tensor destinations cannot satisfy — staging plus a
/// memcpy buys the alignment back, and the copy overlaps the next read.
fn scatter(path: &Path, bytes: u64, threads: usize, chunk: usize) -> Result<u64, String> {
    let segments = tensor_segments(path)?;
    // Arena offsets follow file order here; the real loader packs tensors in
    // its own order, which changes nothing about the copy cost.
    let mut placed = Vec::with_capacity(segments.len());
    let mut arena_len = 0u64;
    for (offset, len) in &segments {
        if arena_len + len > bytes {
            break;
        }
        placed.push((*offset, arena_len, *len));
        arena_len += len;
    }
    platform::scatter_read(path, &placed, arena_len, threads, chunk)
}

/// Tensor (offset, len) pairs from a gguf or safetensors file, in file
/// order — the request stream a real per-tensor load issues.
fn tensor_segments(path: &Path) -> Result<Vec<(u64, u64)>, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "gguf" {
        let gguf = makepad_ai_loader::formats::gguf::GgufFile::open(path)
            .map_err(|e| format!("gguf open: {:?}", e))?;
        let mut segments = gguf
            .tensors
            .iter()
            .map(|t| (gguf.data_offset + t.offset, t.size_bytes))
            .collect::<Vec<_>>();
        segments.sort_by_key(|(offset, _)| *offset);
        Ok(segments)
    } else if ext == "safetensors" {
        let header = makepad_ai_loader::MlxSafetensorsHeader::load(path)
            .map_err(|e| format!("safetensors open: {:?}", e))?;
        let base = header.payload_base_offset();
        let mut segments = header
            .tensors
            .values()
            .map(|entry| {
                let range = entry.file_offsets(base);
                (range[0], range[1] - range[0])
            })
            .collect::<Vec<_>>();
        segments.sort_by_key(|(offset, _)| *offset);
        Ok(segments)
    } else {
        Err(format!("no tensor table for extension '{}'", ext))
    }
}

/// Push the target file out of the OS page cache by streaming a different
/// large file through it with buffered reads (which is what populates the
/// cache and therefore what evicts the previous tenant). Returns bytes
/// streamed.
fn flush_page_cache(other: &str) -> u64 {
    let Ok(meta) = std::fs::metadata(other) else {
        return 0;
    };
    std_chunk(Path::new(other), meta.len(), 8 << 20).unwrap_or(0)
}

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    type Handle = *mut c_void;
    const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
    const MEM_COMMIT: u32 = 0x0000_1000;
    const MEM_RESERVE: u32 = 0x0000_2000;
    const MEM_RELEASE: u32 = 0x0000_8000;
    const PAGE_READWRITE: u32 = 0x04;
    const PAGE_READONLY: u32 = 0x02;
    const ERROR_IO_PENDING: u32 = 997;
    const FILE_MAP_READ: u32 = 0x0004;
    /// NO_BUFFERING requires offsets, lengths and buffer addresses to be
    /// volume-sector multiples. 4096 covers 512e and 4Kn NVMe alike.
    const SECTOR: u64 = 4096;

    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        h_event: Handle,
    }

    extern "system" {
        fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: Handle,
        ) -> Handle;
        fn ReadFile(
            file: Handle,
            buffer: *mut c_void,
            count: u32,
            read: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
        fn SetFilePointerEx(file: Handle, distance: i64, new: *mut i64, method: u32) -> i32;
        fn CreateIoCompletionPort(
            file: Handle,
            existing: Handle,
            key: usize,
            threads: u32,
        ) -> Handle;
        fn GetQueuedCompletionStatus(
            port: Handle,
            bytes: *mut u32,
            key: *mut usize,
            overlapped: *mut *mut Overlapped,
            timeout: u32,
        ) -> i32;
        fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
        fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
        fn CreateFileMappingW(
            file: Handle,
            security: *mut c_void,
            protect: u32,
            high: u32,
            low: u32,
            name: *const u16,
        ) -> Handle;
        fn MapViewOfFile(
            mapping: Handle,
            access: u32,
            offset_high: u32,
            offset_low: u32,
            bytes: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    fn open(path: &Path, flags: u32) -> Result<Handle, String> {
        let name = wide(path);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("CreateFileW failed: {}", unsafe { GetLastError() }));
        }
        Ok(handle)
    }

    /// Page-aligned scratch: NO_BUFFERING rejects unaligned buffers, and
    /// VirtualAlloc is aligned well past the sector requirement.
    struct Aligned {
        ptr: *mut u8,
    }

    impl Aligned {
        fn new(len: usize) -> Result<Self, String> {
            let ptr = unsafe {
                VirtualAlloc(
                    std::ptr::null_mut(),
                    len,
                    MEM_COMMIT | MEM_RESERVE,
                    PAGE_READWRITE,
                )
            };
            if ptr.is_null() {
                return Err(format!("VirtualAlloc({}) failed: {}", len, unsafe {
                    GetLastError()
                }));
            }
            Ok(Self {
                ptr: ptr.cast::<u8>(),
            })
        }
    }

    impl Drop for Aligned {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.ptr.cast::<c_void>(), 0, MEM_RELEASE);
            }
        }
    }

    unsafe impl Send for Aligned {}

    fn align_up(value: u64, to: u64) -> u64 {
        value.div_ceil(to) * to
    }

    pub fn seq_scan(path: &Path, bytes: u64, chunk: usize, arena: bool) -> Result<u64, String> {
        read_sync(path, bytes, chunk, arena, FILE_FLAG_SEQUENTIAL_SCAN)
    }

    pub fn nobuf(path: &Path, bytes: u64, chunk: usize, arena: bool) -> Result<u64, String> {
        read_sync(
            path,
            bytes,
            chunk,
            arena,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN,
        )
    }

    fn read_sync(
        path: &Path,
        bytes: u64,
        chunk: usize,
        arena: bool,
        flags: u32,
    ) -> Result<u64, String> {
        let handle = open(path, flags)?;
        let chunk = align_up(chunk as u64, SECTOR) as usize;
        let scratch = if arena {
            Aligned::new(align_up(bytes, SECTOR) as usize)?
        } else {
            Aligned::new(chunk)?
        };
        let mut done = 0u64;
        while done < bytes {
            let want = align_up((chunk as u64).min(bytes - done), SECTOR).min(chunk as u64) as u32;
            let dst = unsafe {
                if arena {
                    scratch.ptr.add(done as usize)
                } else {
                    scratch.ptr
                }
            };
            let mut read = 0u32;
            let ok = unsafe {
                ReadFile(
                    handle,
                    dst.cast::<c_void>(),
                    want,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                let err = unsafe { GetLastError() };
                unsafe { CloseHandle(handle) };
                return Err(format!("ReadFile failed: {}", err));
            }
            if read == 0 {
                break;
            }
            done += read as u64;
            if (read as u64) < want as u64 {
                break;
            }
        }
        unsafe { CloseHandle(handle) };
        Ok(done)
    }

    /// The method NVMe actually wants: unbuffered reads kept in flight at a
    /// fixed queue depth via an IO completion port. Each slot owns its
    /// OVERLAPPED and (in ring mode) its own aligned landing buffer.
    pub fn nobuf_overlapped(
        path: &Path,
        bytes: u64,
        chunk: usize,
        depth: usize,
        arena: bool,
    ) -> Result<u64, String> {
        let handle = open(
            path,
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED | FILE_FLAG_SEQUENTIAL_SCAN,
        )?;
        let port = unsafe { CreateIoCompletionPort(handle, std::ptr::null_mut(), 0, 0) };
        if port.is_null() {
            unsafe { CloseHandle(handle) };
            return Err(format!("CreateIoCompletionPort failed: {}", unsafe {
                GetLastError()
            }));
        }
        let chunk = align_up(chunk as u64, SECTOR);
        let depth = depth.max(1);

        let arena_buf = if arena {
            Some(Aligned::new(align_up(bytes, SECTOR) as usize)?)
        } else {
            None
        };
        let mut slot_bufs = Vec::new();
        if !arena {
            for _ in 0..depth {
                slot_bufs.push(Aligned::new(chunk as usize)?);
            }
        }
        // Boxed so the addresses handed to the kernel stay put.
        let mut slots = (0..depth)
            .map(|_| {
                Box::new(Overlapped {
                    internal: 0,
                    internal_high: 0,
                    offset: 0,
                    offset_high: 0,
                    h_event: std::ptr::null_mut(),
                })
            })
            .collect::<Vec<_>>();

        let mut next_offset = 0u64;
        let mut inflight = 0usize;
        let mut done = 0u64;

        let issue = |slot: &mut Overlapped,
                     buf: *mut u8,
                     offset: u64,
                     len: u32|
         -> Result<bool, String> {
            slot.internal = 0;
            slot.internal_high = 0;
            slot.offset = (offset & 0xFFFF_FFFF) as u32;
            slot.offset_high = (offset >> 32) as u32;
            slot.h_event = std::ptr::null_mut();
            let mut read = 0u32;
            let ok = unsafe {
                ReadFile(
                    handle,
                    buf.cast::<c_void>(),
                    len,
                    &mut read,
                    slot as *mut Overlapped,
                )
            };
            if ok != 0 {
                // Completed inline; the port still queues a packet.
                return Ok(true);
            }
            let err = unsafe { GetLastError() };
            if err == ERROR_IO_PENDING {
                Ok(true)
            } else {
                Err(format!("ReadFile(overlapped) failed: {}", err))
            }
        };

        for index in 0..depth {
            if next_offset >= bytes {
                break;
            }
            let len = chunk.min(align_up(bytes - next_offset, SECTOR)) as u32;
            let buf = match &arena_buf {
                Some(arena) => unsafe { arena.ptr.add(next_offset as usize) },
                None => slot_bufs[index].ptr,
            };
            issue(&mut slots[index], buf, next_offset, len)?;
            next_offset += len as u64;
            inflight += 1;
        }

        while inflight > 0 {
            let mut transferred = 0u32;
            let mut key = 0usize;
            let mut completed: *mut Overlapped = std::ptr::null_mut();
            let ok = unsafe {
                GetQueuedCompletionStatus(
                    port,
                    &mut transferred,
                    &mut key,
                    &mut completed,
                    60_000,
                )
            };
            if ok == 0 && completed.is_null() {
                let err = unsafe { GetLastError() };
                unsafe { CloseHandle(handle) };
                unsafe { CloseHandle(port) };
                return Err(format!("GetQueuedCompletionStatus failed: {}", err));
            }
            inflight -= 1;
            done += transferred as u64;
            if transferred == 0 || next_offset >= bytes {
                continue;
            }
            // Reissue this slot at the next offset.
            let index = slots
                .iter()
                .position(|slot| std::ptr::eq(slot.as_ref() as *const Overlapped, completed))
                .ok_or_else(|| "completion for unknown slot".to_string())?;
            let len = chunk.min(align_up(bytes - next_offset, SECTOR)) as u32;
            let buf = match &arena_buf {
                Some(arena) => unsafe { arena.ptr.add(next_offset as usize) },
                None => slot_bufs[index].ptr,
            };
            issue(&mut slots[index], buf, next_offset, len)?;
            next_offset += len as u64;
            inflight += 1;
        }

        unsafe { CloseHandle(handle) };
        unsafe { CloseHandle(port) };
        Ok(done.min(bytes))
    }

    /// Raw destination pointer shared across reader threads. Each thread
    /// writes a disjoint byte range, so there is no aliasing.
    #[derive(Clone, Copy)]
    struct DestPtr(*mut u8);
    unsafe impl Send for DestPtr {}

    impl DestPtr {
        /// Taking `self` by value makes a closure capture the whole wrapper
        /// (which is `Send`) instead of precision-capturing the raw pointer
        /// field (which is not).
        fn at(self, offset: usize) -> *mut u8 {
            unsafe { self.0.add(offset) }
        }
    }

    /// Cost of the destination arena alone: allocate `bytes` and touch every
    /// page with `threads` threads. A fresh large allocation is demand-zero,
    /// so the first write to each page traps into the kernel — at 16 GB that
    /// is ~4M soft faults, and single-threaded it is seconds of pure CPU
    /// serialized in front of the read.
    pub fn prefault(bytes: u64, threads: usize) -> Result<u64, String> {
        let threads = threads.max(1);
        let arena = Aligned::new(align_up(bytes, SECTOR) as usize)?;
        let dest = DestPtr(arena.ptr);
        let span = align_up(bytes.div_ceil(threads as u64), SECTOR);
        let mut handles = Vec::new();
        for index in 0..threads {
            let start = span * index as u64;
            if start >= bytes {
                break;
            }
            let len = span.min(bytes - start);
            let dest = dest;
            handles.push(std::thread::spawn(move || {
                let mut offset = 0u64;
                while offset < len {
                    unsafe { *dest.at((start + offset) as usize) = 1 };
                    offset += 4096;
                }
            }));
        }
        for handle in handles {
            handle.join().map_err(|_| "prefault thread panicked")?;
        }
        drop(arena);
        Ok(bytes)
    }

    /// Parallel segments: N threads each stream their own contiguous slice.
    /// In arena mode they land directly in a shared full-size destination,
    /// which spreads the demand-zero fault cost across cores instead of
    /// paying it serially on one.
    pub fn threaded(
        path: &Path,
        bytes: u64,
        threads: usize,
        chunk: usize,
        unbuffered: bool,
        arena: bool,
    ) -> Result<u64, String> {
        let threads = threads.max(1);
        let span = align_up(bytes.div_ceil(threads as u64), SECTOR);
        let flags = if unbuffered {
            FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN
        } else {
            FILE_FLAG_SEQUENTIAL_SCAN
        };
        let arena_buf = if arena {
            Some(Aligned::new(align_up(bytes, SECTOR) as usize)?)
        } else {
            None
        };
        let dest = arena_buf.as_ref().map(|a| DestPtr(a.ptr));
        let mut handles = Vec::new();
        for index in 0..threads {
            let start = span * index as u64;
            if start >= bytes {
                break;
            }
            let len = span.min(bytes - start);
            let path = path.to_path_buf();
            let dest = dest;
            handles.push(std::thread::spawn(move || -> Result<u64, String> {
                let handle = open(&path, flags)?;
                let mut moved = 0i64;
                if unsafe { SetFilePointerEx(handle, start as i64, &mut moved, 0) } == 0 {
                    let err = unsafe { GetLastError() };
                    unsafe { CloseHandle(handle) };
                    return Err(format!("SetFilePointerEx failed: {}", err));
                }
                let chunk = align_up(chunk as u64, SECTOR) as usize;
                let scratch = match dest {
                    Some(_) => None,
                    None => Some(Aligned::new(chunk)?),
                };
                let mut done = 0u64;
                while done < len {
                    let want = (chunk as u64).min(align_up(len - done, SECTOR)) as u32;
                    let mut read = 0u32;
                    let buf = match (&scratch, dest) {
                        (Some(scratch), _) => scratch.ptr,
                        (None, Some(dest)) => dest.at((start + done) as usize),
                        (None, None) => return Err("threaded: no destination".to_string()),
                    };
                    let ok = unsafe {
                        ReadFile(
                            handle,
                            buf.cast::<c_void>(),
                            want,
                            &mut read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 {
                        let err = unsafe { GetLastError() };
                        unsafe { CloseHandle(handle) };
                        return Err(format!("ReadFile failed: {}", err));
                    }
                    if read == 0 {
                        break;
                    }
                    done += read as u64;
                    if (read as u64) < want as u64 {
                        break;
                    }
                }
                unsafe { CloseHandle(handle) };
                Ok(done)
            }));
        }
        let mut total = 0u64;
        for handle in handles {
            total += handle.join().map_err(|_| "reader thread panicked")??;
        }
        Ok(total.min(bytes))
    }

    /// Streams `placed` (file_offset, arena_offset, len) tensors into one
    /// arena with `threads` unbuffered readers. Each thread owns a run of
    /// whole tensors, so no two threads write the same arena bytes.
    pub fn scatter_read(
        path: &Path,
        placed: &[(u64, u64, u64)],
        arena_len: u64,
        threads: usize,
        chunk: usize,
    ) -> Result<u64, String> {
        if placed.is_empty() {
            return Ok(0);
        }
        let threads = threads.max(1);
        let arena = Aligned::new(align_up(arena_len, SECTOR) as usize)?;
        let dest = DestPtr(arena.ptr);
        let chunk = align_up(chunk as u64, SECTOR) as usize;
        let per_thread = placed.len().div_ceil(threads);

        let mut handles = Vec::new();
        for group in placed.chunks(per_thread) {
            let group = group.to_vec();
            let path = path.to_path_buf();
            let dest = dest;
            handles.push(std::thread::spawn(move || -> Result<u64, String> {
                let handle = open(
                    &path,
                    FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN,
                )?;
                let staging = Aligned::new(chunk)?;
                // Sector-align the span this thread streams; the head/tail
                // slop lands in staging and is simply not copied out.
                let first = group[0].0 / SECTOR * SECTOR;
                let last = group
                    .iter()
                    .map(|(offset, _, len)| offset + len)
                    .max()
                    .unwrap_or(first);
                let end = align_up(last, SECTOR);

                let mut moved = 0i64;
                if unsafe { SetFilePointerEx(handle, first as i64, &mut moved, 0) } == 0 {
                    let err = unsafe { GetLastError() };
                    unsafe { CloseHandle(handle) };
                    return Err(format!("SetFilePointerEx failed: {}", err));
                }

                let mut window = first;
                let mut cursor = 0usize;
                let mut copied = 0u64;
                while window < end {
                    let want = (chunk as u64).min(end - window) as u32;
                    let mut read = 0u32;
                    let ok = unsafe {
                        ReadFile(
                            handle,
                            staging.ptr.cast::<c_void>(),
                            want,
                            &mut read,
                            std::ptr::null_mut(),
                        )
                    };
                    if ok == 0 {
                        let err = unsafe { GetLastError() };
                        unsafe { CloseHandle(handle) };
                        return Err(format!("ReadFile failed: {}", err));
                    }
                    if read == 0 {
                        break;
                    }
                    let short = (read as u64) < want as u64;
                    let window_end = window + read as u64;
                    // Copy out every tensor byte this window covers. Tensors
                    // larger than the window are copied piecewise; `cursor`
                    // only advances past tensors that are fully done.
                    let mut index = cursor;
                    while index < group.len() {
                        let (file_offset, arena_offset, len) = group[index];
                        if file_offset >= window_end {
                            break;
                        }
                        let copy_start = file_offset.max(window);
                        let copy_end = (file_offset + len).min(window_end);
                        if copy_end > copy_start {
                            let src_off = (copy_start - window) as usize;
                            let dst_off = (arena_offset + (copy_start - file_offset)) as usize;
                            let n = (copy_end - copy_start) as usize;
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    staging.ptr.add(src_off),
                                    dest.at(dst_off),
                                    n,
                                );
                            }
                            copied += n as u64;
                        }
                        if file_offset + len <= window_end {
                            if index == cursor {
                                cursor += 1;
                            }
                            index += 1;
                        } else {
                            break;
                        }
                    }
                    window = window_end;
                    if short {
                        // EOF: the pointer is now mid-sector, so unbuffered
                        // reads cannot continue (and there is nothing left).
                        break;
                    }
                }
                unsafe { CloseHandle(handle) };
                Ok(copied)
            }));
        }
        let mut total = 0u64;
        for handle in handles {
            total += handle.join().map_err(|_| "scatter thread panicked")??;
        }
        drop(arena);
        Ok(total)
    }

    /// Windows file mapping + page touch — the closest analogue to the unix
    /// mmap path the loader uses on mac.
    pub fn win_mmap(path: &Path, bytes: u64) -> Result<u64, String> {
        let handle = open(path, FILE_FLAG_SEQUENTIAL_SCAN)?;
        let mapping = unsafe {
            CreateFileMappingW(
                handle,
                std::ptr::null_mut(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            )
        };
        if mapping.is_null() {
            let err = unsafe { GetLastError() };
            unsafe { CloseHandle(handle) };
            return Err(format!("CreateFileMappingW failed: {}", err));
        }
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        if view.is_null() {
            let err = unsafe { GetLastError() };
            unsafe { CloseHandle(mapping) };
            unsafe { CloseHandle(handle) };
            return Err(format!("MapViewOfFile failed: {}", err));
        }
        let mut sum = 0u64;
        let base = view.cast::<u8>();
        let mut offset = 0u64;
        while offset < bytes {
            sum += unsafe { *base.add(offset as usize) } as u64;
            offset += 4096;
        }
        std::hint::black_box(sum);
        unsafe { UnmapViewOfFile(view) };
        unsafe { CloseHandle(mapping) };
        unsafe { CloseHandle(handle) };
        Ok(bytes)
    }
}

#[cfg(not(windows))]
mod platform {
    use std::path::Path;

    const NOT_WINDOWS: &str = "windows-only method";

    pub fn seq_scan(_p: &Path, _b: u64, _c: usize, _a: bool) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn nobuf(_p: &Path, _b: u64, _c: usize, _a: bool) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn nobuf_overlapped(
        _p: &Path,
        _b: u64,
        _c: usize,
        _d: usize,
        _a: bool,
    ) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn threaded(
        _p: &Path,
        _b: u64,
        _t: usize,
        _c: usize,
        _u: bool,
        _a: bool,
    ) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn prefault(_b: u64, _t: usize) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn scatter_read(
        _p: &Path,
        _pl: &[(u64, u64, u64)],
        _a: u64,
        _t: usize,
        _c: usize,
    ) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
    pub fn win_mmap(_p: &Path, _b: u64) -> Result<u64, String> {
        Err(NOT_WINDOWS.to_string())
    }
}
