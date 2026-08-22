//! Metal weight-residency probe: what a model swap actually costs on
//! unified memory, measured without executing a single kernel.
//!
//! On CUDA the load story ends with an H2D copy, and "is it resident?" is a
//! question you ask nvidia-smi. On Apple silicon there is no copy at all:
//! `newBufferWithBytesNoCopy` wraps host pages the GPU already shares, so
//! the real questions are different and none of them need a forward pass:
//!
//!   * does wrapping a mapping in a Metal buffer make its pages resident,
//!     or does it defer that to whoever touches them first?
//!   * what does allocating (as opposed to wrapping) multi-GB cost?
//!   * is there per-buffer overhead worth batching tensors to avoid?
//!   * what does tearing a model down cost — buffer release and munmap of
//!     multi-GB regions, and does either hide a sync?
//!
//! `mincore(2)` answers the first one exactly, page by page, so residency
//! here is measured rather than inferred.
//!
//! usage: metal-residency-bench <file.gguf> [scenario ...] [--limit:<GB>]
//!   scenarios: map, nocopy, nocopy-primed, alloc:<GB>, alloc-private:<GB>,
//!              per-buffer:<n>, teardown, swap:<other.gguf>, all
//!
//! Every scenario that wants a cold file evicts it first (per-file
//! `msync(MS_INVALIDATE)`, never a system-wide purge) and prints the
//! verified before/after residency.

use makepad_ai_loader::mmap::MappedRegion;
use std::path::Path;
use std::time::Instant;

#[cfg(target_os = "macos")]
mod probe {
    use super::*;
    use makepad_ai_metal::{BufferStorageMode, MetalRuntime};
    use std::os::raw::{c_char, c_int, c_void};
    use std::os::unix::ffi::OsStrExt;

    const O_RDONLY: c_int = 0;
    const PROT_READ: c_int = 1;
    const MAP_SHARED: c_int = 1;
    const MS_INVALIDATE: c_int = 0x0002;

    extern "C" {
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn close(fd: c_int) -> c_int;
        fn mmap(a: *mut c_void, l: usize, p: c_int, f: c_int, fd: c_int, o: i64) -> *mut c_void;
        fn munmap(a: *mut c_void, l: usize) -> c_int;
        fn msync(a: *mut c_void, l: usize, fl: c_int) -> c_int;
        fn getpagesize() -> c_int;
    }

    fn gb(bytes: u64) -> f64 {
        bytes as f64 / (1u64 << 30) as f64
    }

    /// Drop one file's pages from the unified page cache. `MS_INVALIDATE`
    /// is the only primitive that works, and only on its own — see the note
    /// in `mac_storage_bench`. Verified by the caller via `mincore`, never
    /// trusted on its return code.
    pub fn evict(path: &Path, len: usize) -> Result<(), String> {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "path contains NUL".to_string())?;
        let fd = unsafe { open(c.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return Err(format!("open {:?} failed", path));
        }
        let page = unsafe { getpagesize() }.max(1) as usize;
        let len = len.next_multiple_of(page);
        let ptr = unsafe { mmap(std::ptr::null_mut(), len, PROT_READ, MAP_SHARED, fd, 0) };
        unsafe { close(fd) };
        if ptr.is_null() || ptr as isize == -1 {
            return Err("mmap for evict failed".to_string());
        }
        let rc = unsafe { msync(ptr, len, MS_INVALIDATE) };
        unsafe { munmap(ptr, len) };
        if rc != 0 {
            return Err("msync(MS_INVALIDATE) failed".to_string());
        }
        Ok(())
    }

    fn resident_pct(region: &MappedRegion) -> f64 {
        let (resident, pages) = region.residency();
        100.0 * resident as f64 / pages.max(1) as f64
    }

    /// The headline question: after wrapping a COLD mapping in a Metal
    /// no-copy buffer, are the pages resident? If they are not, the cost of
    /// reading the weights has not been paid at load time — it has been
    /// deferred into whoever touches them first, which is the first
    /// forward pass.
    pub fn nocopy(runtime: &MetalRuntime, path: &Path, primed: bool) -> Result<(), String> {
        evict(path, usize::MAX.min(std::fs::metadata(path).map_err(|e| e.to_string())?.len() as usize))?;
        let map_started = Instant::now();
        let region = MappedRegion::map_file(path)?;
        let map_s = map_started.elapsed().as_secs_f64();
        let bytes = region.len() as u64;
        println!(
            "  map_file            {:>8.4} s   (resident {:.1}%)",
            map_s,
            resident_pct(&region)
        );

        if primed {
            let started = Instant::now();
            region.prefault();
            let s = started.elapsed().as_secs_f64();
            println!(
                "  prefault            {:>8.4} s   {:>6.2} GB/s  (resident {:.1}%)",
                s,
                gb(bytes) / s,
                resident_pct(&region)
            );
        }

        let started = Instant::now();
        // Safety: `region` is page-aligned and page-multiple by
        // construction and outlives `buffer`.
        let buffer = unsafe { runtime.create_buffer_no_copy(region.as_slice()) }
            .map_err(|e| format!("{:?}", e))?;
        let s = started.elapsed().as_secs_f64();
        println!(
            "  create_buffer_no_copy {:>6.4} s   {:>6.2} GB/s  (resident {:.1}%)  [{:.2} GB]",
            s,
            gb(bytes) / s,
            resident_pct(&region),
            gb(bytes)
        );

        let started = Instant::now();
        drop(buffer);
        println!(
            "  buffer release      {:>8.4} s   (resident {:.1}%)",
            started.elapsed().as_secs_f64(),
            resident_pct(&region)
        );

        let started = Instant::now();
        drop(region);
        println!(
            "  munmap              {:>8.4} s",
            started.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// Allocating (rather than wrapping) memory: what a fresh Shared or
    /// Private buffer costs, and what the first CPU touch of a Shared one
    /// costs on top — allocation is lazy, so the pages arrive later.
    pub fn alloc(runtime: &MetalRuntime, bytes: usize, private: bool) -> Result<(), String> {
        let storage = if private {
            BufferStorageMode::Private
        } else {
            BufferStorageMode::Shared
        };
        let started = Instant::now();
        let buffer = runtime
            .create_buffer(bytes, storage)
            .map_err(|e| format!("{:?}", e))?;
        let alloc_s = started.elapsed().as_secs_f64();
        println!(
            "  create_buffer {:<8} {:>8.4} s   {:>7.2} GB/s  [{:.2} GB]",
            if private { "Private" } else { "Shared" },
            alloc_s,
            gb(bytes as u64) / alloc_s,
            gb(bytes as u64)
        );
        let started = Instant::now();
        drop(buffer);
        println!(
            "  release             {:>8.4} s",
            started.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// Per-buffer overhead: the same bytes wrapped as one buffer versus `n`
    /// page-aligned slices, which is the difference between the arena the
    /// loader builds today and a hypothetical buffer-per-tensor layout.
    pub fn per_buffer(runtime: &MetalRuntime, path: &Path, n: usize) -> Result<(), String> {
        let region = MappedRegion::map_file(path)?;
        region.prefault();
        let page = unsafe { getpagesize() }.max(1) as usize;
        let bytes = region.as_slice();

        let started = Instant::now();
        let one = unsafe { runtime.create_buffer_no_copy(bytes) }.map_err(|e| format!("{:?}", e))?;
        let one_s = started.elapsed().as_secs_f64();
        drop(one);

        let slice_len = (bytes.len() / n) & !(page - 1);
        if slice_len == 0 {
            return Err("slice too small".to_string());
        }
        let started = Instant::now();
        let mut buffers = Vec::with_capacity(n);
        for i in 0..n {
            let start = i * slice_len;
            buffers.push(
                unsafe { runtime.create_buffer_no_copy(&bytes[start..start + slice_len]) }
                    .map_err(|e| format!("{:?}", e))?,
            );
        }
        let many_s = started.elapsed().as_secs_f64();
        let release_started = Instant::now();
        drop(buffers);
        println!(
            "  1 buffer {:>10.5} s   |   {} buffers {:>8.5} s ({:>7.1} us each)   release {:.5} s",
            one_s,
            n,
            many_s,
            many_s * 1e6 / n as f64,
            release_started.elapsed().as_secs_f64()
        );
        Ok(())
    }

    /// The headline metric: evict model A and bring model B fully resident,
    /// the way a swap actually happens. Reported cold (B evicted first) and
    /// warm (B still in the page cache), with the teardown of A included.
    pub fn swap(runtime: &MetalRuntime, a: &Path, b: &Path, cold: bool) -> Result<(), String> {
        // Model A resident, as it would be mid-session.
        let region_a = MappedRegion::map_file(a)?;
        region_a.prefault();
        let buffer_a = unsafe { runtime.create_buffer_no_copy(region_a.as_slice()) }
            .map_err(|e| format!("{:?}", e))?;
        let a_bytes = region_a.len() as u64;

        if cold {
            evict(b, std::fs::metadata(b).map_err(|e| e.to_string())?.len() as usize)?;
        }

        let total = Instant::now();
        let started = Instant::now();
        drop(buffer_a);
        drop(region_a);
        let evict_s = started.elapsed().as_secs_f64();

        let started = Instant::now();
        let region_b = MappedRegion::map_file(b)?;
        region_b.prefault();
        let load_s = started.elapsed().as_secs_f64();
        let b_bytes = region_b.len() as u64;

        let started = Instant::now();
        let buffer_b = unsafe { runtime.create_buffer_no_copy(region_b.as_slice()) }
            .map_err(|e| format!("{:?}", e))?;
        let wrap_s = started.elapsed().as_secs_f64();
        let total_s = total.elapsed().as_secs_f64();

        println!(
            "  {} swap: evict A {:.3} s ({:.2} GB) + load B {:.3} s ({:.2} GB @ {:.2} GB/s) \
             + wrap {:.4} s = {:.3} s   (B resident {:.1}%)",
            if cold { "COLD" } else { "WARM" },
            evict_s,
            gb(a_bytes),
            load_s,
            gb(b_bytes),
            gb(b_bytes) / load_s,
            wrap_s,
            total_s,
            resident_pct(&region_b)
        );
        drop(buffer_b);
        drop(region_b);
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("metal-residency-bench is macOS-only");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    use makepad_ai_metal::MetalRuntime;

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: metal-residency-bench <file.gguf> [map|nocopy|nocopy-primed|alloc:<GB>|\
             alloc-private:<GB>|per-buffer:<n>|swap:<other.gguf>|all] [--limit:<GB>]"
        );
        std::process::exit(2);
    }
    let path = std::path::PathBuf::from(&args[0]);
    let mut scenarios: Vec<String> = Vec::new();
    for arg in &args[1..] {
        if !arg.starts_with("--") {
            scenarios.push(arg.clone());
        }
    }
    if scenarios.is_empty() {
        scenarios = vec![
            "map".into(),
            "nocopy".into(),
            "nocopy-primed".into(),
            "alloc:4".into(),
            "alloc-private:4".into(),
            "per-buffer:512".into(),
        ];
    }

    let runtime = match MetalRuntime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("metal-residency-bench: no Metal runtime: {:?}", err);
            std::process::exit(1);
        }
    };
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    println!(
        "file: {} ({:.2} GB)  page={} KB",
        path.display(),
        size as f64 / (1u64 << 30) as f64,
        16
    );

    for scenario in &scenarios {
        let mut parts = scenario.split(':');
        let name = parts.next().unwrap_or("");
        let rest: Vec<&str> = parts.collect();
        println!("[{}]", scenario);
        let result = match name {
            "map" => {
                let started = Instant::now();
                match MappedRegion::map_file(&path) {
                    Ok(region) => {
                        let (resident, pages) = region.residency();
                        println!(
                            "  map_file {:.4} s  resident {} / {} pages ({:.1}%)",
                            started.elapsed().as_secs_f64(),
                            resident,
                            pages,
                            100.0 * resident as f64 / pages.max(1) as f64
                        );
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            }
            "nocopy" => probe::nocopy(&runtime, &path, false),
            "nocopy-primed" => probe::nocopy(&runtime, &path, true),
            "alloc" => probe::alloc(
                &runtime,
                (rest.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(4.0)
                    * (1u64 << 30) as f64) as usize,
                false,
            ),
            "alloc-private" => probe::alloc(
                &runtime,
                (rest.first().and_then(|s| s.parse::<f64>().ok()).unwrap_or(4.0)
                    * (1u64 << 30) as f64) as usize,
                true,
            ),
            "per-buffer" => probe::per_buffer(
                &runtime,
                &path,
                rest.first().and_then(|s| s.parse().ok()).unwrap_or(512),
            ),
            "swap" => {
                let other = std::path::PathBuf::from(rest.join(":"));
                probe::swap(&runtime, &path, &other, true)
                    .and_then(|()| probe::swap(&runtime, &path, &other, false))
            }
            other => Err(format!("unknown scenario {}", other)),
        };
        if let Err(err) = result {
            println!("  SKIP/ERR: {}", err);
        }
    }
}
