//! Bulk weight reader: fills a model arena from disk at device speed.
//!
//! The obvious way to load weights — for each tensor, seek to its offset and
//! read it — issues one small synchronous request at a time. An NVMe drive
//! only reaches its rated bandwidth when several requests are in flight at
//! once, so that shape leaves most of the device idle. Measured on a WD
//! SN850X (rated 7.3 GB/s) with a 16 GB gguf: per-tensor reads 2.03 GB/s,
//! this reader 5.3 GB/s, the drive's own ceiling 6.8 GB/s.
//!
//! The strategy is to stop treating tensors as the I/O unit. Work is split
//! across threads by contiguous runs of tensors; each thread streams its
//! slice of the file in large block reads and copies the tensor bytes out to
//! their arena destinations. On Windows the reads are unbuffered
//! (`FILE_FLAG_NO_BUFFERING`), which skips the cache manager's copy but
//! demands sector-aligned offsets, lengths and buffers — hence the staging
//! block: the alignment lives there, and the scatter memcpy (which overlaps
//! the next read) puts bytes wherever the arena wants them.
//!
//! `MAKEPAD_LOADER_BULK=0` falls back to the plain sequential reader.

use std::path::Path;

/// One tensor's placement: `len` bytes at `file_offset` in the file land at
/// `dst_offset` in the arena.
#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub file_offset: u64,
    pub dst_offset: u64,
    pub len: u64,
}

/// Below this the thread and staging setup costs more than it saves.
const BULK_THRESHOLD: u64 = 64 << 20;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn bulk_enabled() -> bool {
    !matches!(std::env::var("MAKEPAD_LOADER_BULK").as_deref(), Ok("0"))
}

/// Default reader threads. Past ~8 the drive is already saturated; the extra
/// threads only help spread the arena's demand-zero page faults.
fn default_threads() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    env_usize("MAKEPAD_LOADER_THREADS", cpus.clamp(2, 8))
}

fn default_chunk() -> usize {
    env_usize("MAKEPAD_LOADER_CHUNK_MB", 8) << 20
}

/// Fill `arena` from `path` per `placed`. Placements must be inside the
/// arena and must not overlap each other — both are checked here, because
/// the threaded path relies on disjointness for soundness.
pub fn read_placed(path: &Path, arena: &mut [u8], placed: &[Placement]) -> Result<(), String> {
    if placed.is_empty() {
        return Ok(());
    }
    let total = validate(arena.len(), placed)?;

    if !bulk_enabled() || total < BULK_THRESHOLD || placed.len() < 2 {
        return read_sequential(path, arena, placed);
    }

    // File order keeps each thread's reads moving forward through the file.
    let mut ordered = placed.to_vec();
    ordered.sort_unstable_by_key(|p| p.file_offset);

    match imp::read_threaded(path, arena, &ordered, default_threads(), default_chunk()) {
        Ok(()) => Ok(()),
        // Any platform refusal (unsupported flags, odd filesystem) falls back
        // to the path that always works rather than failing the load.
        Err(err) => {
            eprintln!("loader: bulk read fell back to sequential ({})", err);
            read_sequential(path, arena, placed)
        }
    }
}

/// Returns the total byte count, erroring if anything is out of bounds or
/// two placements would write the same arena byte.
fn validate(arena_len: usize, placed: &[Placement]) -> Result<u64, String> {
    let mut total = 0u64;
    let mut ranges = Vec::with_capacity(placed.len());
    for p in placed {
        let end = p
            .dst_offset
            .checked_add(p.len)
            .ok_or_else(|| "bulk read: destination range overflows".to_string())?;
        if end > arena_len as u64 {
            return Err(format!(
                "bulk read: destination [{}..{}) exceeds arena length {}",
                p.dst_offset, end, arena_len
            ));
        }
        p.file_offset
            .checked_add(p.len)
            .ok_or_else(|| "bulk read: file range overflows".to_string())?;
        total += p.len;
        ranges.push((p.dst_offset, end));
    }
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err(format!(
                "bulk read: destinations [{}..{}) and [{}..{}) overlap",
                pair[0].0, pair[0].1, pair[1].0, pair[1].1
            ));
        }
    }
    Ok(total)
}

/// One handle, seek + read per placement. The reference behaviour: every
/// fast path must produce exactly these bytes.
fn read_sequential(path: &Path, arena: &mut [u8], placed: &[Placement]) -> Result<(), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).map_err(|err| format!("{}: {}", path.display(), err))?;
    let mut ordered = placed.to_vec();
    ordered.sort_unstable_by_key(|p| p.file_offset);
    for p in &ordered {
        let start = p.dst_offset as usize;
        let end = start + p.len as usize;
        file.seek(SeekFrom::Start(p.file_offset))
            .map_err(|err| format!("{}: {}", path.display(), err))?;
        file.read_exact(&mut arena[start..end])
            .map_err(|err| format!("{}: {}", path.display(), err))?;
    }
    Ok(())
}

/// Raw arena pointer handed to reader threads. Each thread writes only the
/// placements it owns, and `validate` has already proven those byte ranges
/// are disjoint, so no two threads can touch the same byte.
#[derive(Clone, Copy)]
struct ArenaPtr(*mut u8);

// Safety: the pointer addresses a live `&mut [u8]` that outlives every
// reader thread (they are joined before `read_threaded` returns), and the
// ranges written through it are disjoint per `validate`.
unsafe impl Send for ArenaPtr {}

impl ArenaPtr {
    /// By-value `self` makes closures capture the wrapper (which is `Send`)
    /// rather than precision-capturing the raw pointer field (which is not).
    fn at(self, offset: usize) -> *mut u8 {
        unsafe { self.0.add(offset) }
    }
}

#[cfg(windows)]
mod imp {
    use super::{ArenaPtr, Placement};
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    type Handle = *mut c_void;
    const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const OPEN_EXISTING: u32 = 3;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
    const FILE_BEGIN: u32 = 0;
    const MEM_COMMIT: u32 = 0x0000_1000;
    const MEM_RESERVE: u32 = 0x0000_2000;
    const MEM_RELEASE: u32 = 0x0000_8000;
    const PAGE_READWRITE: u32 = 0x04;
    /// Unbuffered I/O is expressed in whole sectors. 4096 satisfies both
    /// 512e and 4Kn NVMe geometry.
    const SECTOR: u64 = 4096;

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
            overlapped: *mut c_void,
        ) -> i32;
        fn SetFilePointerEx(file: Handle, distance: i64, new: *mut i64, method: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetLastError() -> u32;
        fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32)
            -> *mut c_void;
        fn VirtualFree(address: *mut c_void, size: usize, kind: u32) -> i32;
    }

    /// Sector-aligned staging block. `VirtualAlloc` aligns to 64 KB, well
    /// past what unbuffered reads require.
    struct Staging {
        ptr: *mut u8,
    }

    impl Staging {
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

    impl Drop for Staging {
        fn drop(&mut self) {
            unsafe {
                VirtualFree(self.ptr.cast::<c_void>(), 0, MEM_RELEASE);
            }
        }
    }

    fn align_up(value: u64, to: u64) -> u64 {
        value.div_ceil(to) * to
    }

    fn open(path: &Path) -> Result<Handle, String> {
        let name = path
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<u16>>();
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("CreateFileW failed: {}", unsafe { GetLastError() }));
        }
        Ok(handle)
    }

    pub fn read_threaded(
        path: &Path,
        arena: &mut [u8],
        ordered: &[Placement],
        threads: usize,
        chunk: usize,
    ) -> Result<(), String> {
        let arena_ptr = ArenaPtr(arena.as_mut_ptr());
        let chunk = align_up(chunk as u64, SECTOR) as usize;
        // Split by bytes, not by tensor count: a few huge experts next to
        // many tiny norms would otherwise leave threads idle.
        let total: u64 = ordered.iter().map(|p| p.len).sum();
        let target = total.div_ceil(threads.max(1) as u64);

        let mut groups: Vec<Vec<Placement>> = Vec::new();
        let mut current: Vec<Placement> = Vec::new();
        let mut current_bytes = 0u64;
        for p in ordered {
            current.push(*p);
            current_bytes += p.len;
            if current_bytes >= target && groups.len() + 1 < threads.max(1) {
                groups.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
        }
        if !current.is_empty() {
            groups.push(current);
        }

        let mut handles = Vec::with_capacity(groups.len());
        for group in groups {
            let path = path.to_path_buf();
            let arena_ptr = arena_ptr;
            handles.push(std::thread::spawn(move || -> Result<(), String> {
                read_group(&path, arena_ptr, &group, chunk)
            }));
        }
        let mut first_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    first_error.get_or_insert(err);
                }
                Err(_) => {
                    first_error.get_or_insert_with(|| "reader thread panicked".to_string());
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Streams the file span covering `group` and copies each placement's
    /// bytes into the arena. Placements are in ascending file order.
    fn read_group(
        path: &Path,
        arena: ArenaPtr,
        group: &[Placement],
        chunk: usize,
    ) -> Result<(), String> {
        let Some(first) = group.first() else {
            return Ok(());
        };
        let handle = open(path)?;
        let result = read_group_inner(handle, arena, group, chunk, first.file_offset);
        unsafe { CloseHandle(handle) };
        result
    }

    fn read_group_inner(
        handle: Handle,
        arena: ArenaPtr,
        group: &[Placement],
        chunk: usize,
        first_offset: u64,
    ) -> Result<(), String> {
        // Round the span out to sector boundaries; the slop read at either
        // end simply is not copied out.
        let start = first_offset / SECTOR * SECTOR;
        let last = group
            .iter()
            .map(|p| p.file_offset + p.len)
            .max()
            .unwrap_or(start);
        let end = align_up(last, SECTOR);

        let staging = Staging::new(chunk)?;
        let mut moved = 0i64;
        if unsafe { SetFilePointerEx(handle, start as i64, &mut moved, FILE_BEGIN) } == 0 {
            return Err(format!("SetFilePointerEx failed: {}", unsafe {
                GetLastError()
            }));
        }

        let mut window = start;
        // Placements fully copied so far; those before it can be skipped.
        let mut done = 0usize;
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
                return Err(format!("ReadFile failed: {}", unsafe { GetLastError() }));
            }
            if read == 0 {
                break;
            }
            let window_end = window + read as u64;

            let mut index = done;
            while index < group.len() {
                let p = group[index];
                if p.file_offset >= window_end {
                    break;
                }
                let copy_start = p.file_offset.max(window);
                let copy_end = (p.file_offset + p.len).min(window_end);
                if copy_end > copy_start {
                    let src = (copy_start - window) as usize;
                    let dst = (p.dst_offset + (copy_start - p.file_offset)) as usize;
                    let len = (copy_end - copy_start) as usize;
                    // Safety: `src..src+len` is inside the staging block
                    // (copy_end <= window_end = window + read <= window +
                    // chunk), and `dst..dst+len` is inside the arena and
                    // owned solely by this thread (checked in `validate`).
                    debug_assert!(src + len <= chunk);
                    unsafe {
                        std::ptr::copy_nonoverlapping(staging.ptr.add(src), arena.at(dst), len);
                    }
                }
                if p.file_offset + p.len <= window_end {
                    if index == done {
                        done += 1;
                    }
                    index += 1;
                } else {
                    break;
                }
            }

            window = window_end;
            if (read as u64) < want as u64 {
                // Short read means EOF: the file pointer is now mid-sector,
                // so no further unbuffered read can be issued from it.
                break;
            }
        }

        if done < group.len() {
            return Err(format!(
                "bulk read: {} of {} tensors unfilled (file ended early)",
                group.len() - done,
                group.len()
            ));
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{ArenaPtr, Placement};
    use std::os::raw::{c_int, c_void};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    extern "C" {
        fn open(path: *const std::os::raw::c_char, flags: c_int, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        fn pread(fd: c_int, buf: *mut c_void, count: usize, offset: i64) -> isize;
    }

    const O_RDONLY: c_int = 0;

    /// Positional reads need no shared file offset, so threads can share one
    /// descriptor and read their own placements concurrently.
    pub fn read_threaded(
        path: &Path,
        arena: &mut [u8],
        ordered: &[Placement],
        threads: usize,
        _chunk: usize,
    ) -> Result<(), String> {
        let arena_ptr = ArenaPtr(arena.as_mut_ptr());
        let total: u64 = ordered.iter().map(|p| p.len).sum();
        let target = total.div_ceil(threads.max(1) as u64);

        let mut groups: Vec<Vec<Placement>> = Vec::new();
        let mut current: Vec<Placement> = Vec::new();
        let mut current_bytes = 0u64;
        for p in ordered {
            current.push(*p);
            current_bytes += p.len;
            if current_bytes >= target && groups.len() + 1 < threads.max(1) {
                groups.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
        }
        if !current.is_empty() {
            groups.push(current);
        }

        let mut handles = Vec::with_capacity(groups.len());
        for group in groups {
            let path = path.to_path_buf();
            let arena_ptr = arena_ptr;
            handles.push(std::thread::spawn(move || -> Result<(), String> {
                let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
                    .map_err(|_| format!("path {:?} contains a NUL byte", path))?;
                let fd = unsafe { open(c_path.as_ptr(), O_RDONLY) };
                if fd < 0 {
                    return Err(format!("open {:?} failed", path));
                }
                let result = read_group(fd, arena_ptr, &group);
                unsafe { close(fd) };
                result
            }));
        }
        let mut first_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    first_error.get_or_insert(err);
                }
                Err(_) => {
                    first_error.get_or_insert_with(|| "reader thread panicked".to_string());
                }
            }
        }
        match first_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn read_group(fd: c_int, arena: ArenaPtr, group: &[Placement]) -> Result<(), String> {
        for p in group {
            let mut done = 0u64;
            while done < p.len {
                let remaining = (p.len - done) as usize;
                // Safety: `dst_offset + len` is inside the arena and this
                // thread owns the range exclusively (checked in `validate`).
                let dst = arena.at((p.dst_offset + done) as usize);
                let got = unsafe {
                    pread(
                        fd,
                        dst.cast::<c_void>(),
                        remaining,
                        (p.file_offset + done) as i64,
                    )
                };
                if got < 0 {
                    return Err("pread failed".to_string());
                }
                if got == 0 {
                    return Err("bulk read: unexpected end of file".to_string());
                }
                done += got as u64;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(bytes: &[u8]) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "makepad-bulk-read-{}-{:?}.bin",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let mut file = std::fs::File::create(&path).expect("create temp file");
        file.write_all(bytes).expect("write temp file");
        path
    }

    /// The fast path must land exactly the bytes the sequential reader
    /// would, including when placements are out of file order and the arena
    /// has gaps between them.
    #[test]
    fn threaded_matches_sequential() {
        let source = (0..(4 << 20) as u32)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<u8>>();
        let path = temp_file(&source);

        // Scattered placements: reversed arena order, with padding gaps.
        let mut placed = Vec::new();
        let mut dst = 7u64;
        let chunk = 97_003u64;
        let mut file_offset = 11u64;
        while file_offset + chunk < source.len() as u64 {
            placed.push(Placement {
                file_offset,
                dst_offset: dst,
                len: chunk,
            });
            file_offset += chunk + 13;
            dst += chunk + 5;
        }
        let arena_len = (dst + 64) as usize;

        let mut expected = vec![0u8; arena_len];
        read_sequential(&path, &mut expected, &placed).expect("sequential read");

        let mut actual = vec![0u8; arena_len];
        // Force the threaded path regardless of size heuristics.
        let mut ordered = placed.clone();
        ordered.sort_unstable_by_key(|p| p.file_offset);
        imp::read_threaded(&path, &mut actual, &ordered, 4, 64 << 10)
            .expect("threaded read");

        assert_eq!(actual, expected, "threaded bytes differ from sequential");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overlapping_destinations_are_rejected() {
        let placed = [
            Placement {
                file_offset: 0,
                dst_offset: 0,
                len: 100,
            },
            Placement {
                file_offset: 200,
                dst_offset: 50,
                len: 100,
            },
        ];
        assert!(validate(1000, &placed).is_err());
    }

    #[test]
    fn out_of_bounds_destination_is_rejected() {
        let placed = [Placement {
            file_offset: 0,
            dst_offset: 900,
            len: 200,
        }];
        assert!(validate(1000, &placed).is_err());
    }
}
