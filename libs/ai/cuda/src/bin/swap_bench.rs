//! Model-swap cost probe: the host->device half of a load, and the teardown
//! half of an evict.
//!
//! A swap on a memory-tight card is evict(A) + load(B) + first inference.
//! This bin measures the pieces the storage probe cannot see:
//!   * H2D bandwidth from pageable vs pinned host memory, at various chunk
//!     sizes — pageable copies are staged by the driver through a small
//!     internal pinned buffer and cannot reach link speed.
//!   * a read->stage->upload pipeline, serial vs overlapped, to show what
//!     hiding the disk behind the copy engine is worth.
//!   * teardown: how long it takes to free N device buffers and drop a
//!     multi-GB host arena — the "evict is slow" question.
//!
//! usage: swap-bench <mode> [args]
//!   h2d <GB>              pageable vs pinned H2D at 1/4/16/64 MB chunks
//!   pipeline <file> <GB>  serial vs overlapped disk->pinned->device
//!   evict <GB> <buffers>  allocate then free, phase by phase
//!   pinned <GB>           cost of allocating/freeing pinned host memory

use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Instant;

use makepad_ai_cuda as cuda;

const GB: usize = 1 << 30;
const MB: usize = 1 << 20;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        eprintln!("usage: swap-bench <h2d|pipeline|evict|pinned> [args]");
        std::process::exit(2);
    }
    if let Err(err) = cuda::set_device(0) {
        eprintln!("swap-bench: cudaSetDevice(0) failed: {}", err);
        std::process::exit(1);
    }
    let (free, total) = cuda::mem_get_info().unwrap_or((0, 0));
    println!(
        "device 0: {:.1} GB free / {:.1} GB total",
        free as f64 / GB as f64,
        total as f64 / GB as f64
    );

    let result = match args[0].as_str() {
        "h2d" => h2d(arg_f64(&args, 1, 4.0)),
        "pipeline" => pipeline(&args[1], arg_f64(&args, 2, 8.0)),
        "evict" => evict(arg_f64(&args, 1, 8.0), arg_usize(&args, 2, 512)),
        "pinned" => pinned(arg_f64(&args, 1, 4.0)),
        other => Err(format!("unknown mode '{}'", other)),
    };
    if let Err(err) = result {
        eprintln!("swap-bench: {}", err);
        std::process::exit(1);
    }
}

fn arg_f64(args: &[String], index: usize, default: f64) -> f64 {
    args.get(index)
        .and_then(|a| a.parse::<f64>().ok())
        .unwrap_or(default)
}

fn arg_usize(args: &[String], index: usize, default: usize) -> usize {
    args.get(index)
        .and_then(|a| a.parse::<usize>().ok())
        .unwrap_or(default)
}

fn gb_of(bytes: usize) -> f64 {
    bytes as f64 / GB as f64
}

/// Host->device bandwidth, pageable source vs page-locked source. The
/// driver cannot DMA straight out of pageable pages, so it copies them into
/// its own staging buffer first; the difference is the cost of that.
fn h2d(size_gb: f64) -> Result<(), String> {
    let bytes = (size_gb * GB as f64) as usize;
    let stream = cuda::create_non_blocking_stream().map_err(|e| e.to_string())?;
    let device = unsafe { cuda::malloc(bytes) }.map_err(|e| e.to_string())?;

    let pageable = vec![0u8; bytes];
    let started = Instant::now();
    let pinned = unsafe { cuda::host_alloc_pinned(bytes) }.map_err(|e| e.to_string())?;
    println!(
        "pinned alloc {:.2} GB: {:.3} s ({:.2} GB/s)",
        gb_of(bytes),
        started.elapsed().as_secs_f64(),
        gb_of(bytes) / started.elapsed().as_secs_f64()
    );

    for chunk_mb in [1usize, 4, 16, 64, 256] {
        let chunk = chunk_mb * MB;
        for (label, src) in [
            ("pageable", pageable.as_ptr()),
            ("pinned  ", pinned.as_ptr().cast::<u8>()),
        ] {
            // Warm the path once so the first-call driver setup is not timed.
            copy_chunks(device, src, bytes.min(chunk), chunk, stream)?;
            let started = Instant::now();
            copy_chunks(device, src, bytes, chunk, stream)?;
            let elapsed = started.elapsed().as_secs_f64();
            println!(
                "h2d {} chunk={:>4} MB: {:>6.3} s  {:>6.2} GB/s",
                label,
                chunk_mb,
                elapsed,
                gb_of(bytes) / elapsed
            );
        }
    }

    unsafe { cuda::free_host(pinned) }.map_err(|e| e.to_string())?;
    unsafe { cuda::free(device) }.map_err(|e| e.to_string())?;
    cuda::destroy_stream(stream).map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_chunks(
    device: NonNull<c_void>,
    src: *const u8,
    bytes: usize,
    chunk: usize,
    stream: cuda::cudaStream_t,
) -> Result<(), String> {
    let mut offset = 0usize;
    while offset < bytes {
        let n = chunk.min(bytes - offset);
        let dst = unsafe {
            NonNull::new_unchecked(device.as_ptr().cast::<u8>().add(offset).cast::<c_void>())
        };
        unsafe {
            cuda::memcpy_async_host_to_device(dst, src.add(offset).cast::<c_void>(), n, stream)
        }
        .map_err(|e| e.to_string())?;
        offset += n;
    }
    cuda::synchronize_stream(stream).map_err(|e| e.to_string())
}

/// Disk -> pinned staging -> device, serial vs double-buffered. Serial pays
/// read + upload back to back; overlapped pays max(read, upload) because the
/// copy engine runs while the next block is being read.
fn pipeline(path: &str, size_gb: f64) -> Result<(), String> {
    use std::io::Read;
    let bytes = (size_gb * GB as f64) as usize;
    let stream = cuda::create_non_blocking_stream().map_err(|e| e.to_string())?;
    let device = unsafe { cuda::malloc(bytes) }.map_err(|e| e.to_string())?;
    let chunk = 32 * MB;

    // Serial: one staging block, read it, upload it, repeat.
    let staging = unsafe { cuda::host_alloc_pinned(chunk) }.map_err(|e| e.to_string())?;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut offset = 0usize;
    let mut read_time = 0f64;
    let mut upload_time = 0f64;
    while offset < bytes {
        let n = chunk.min(bytes - offset);
        let host =
            unsafe { std::slice::from_raw_parts_mut(staging.as_ptr().cast::<u8>(), n) };
        let read_started = Instant::now();
        file.read_exact(host).map_err(|e| e.to_string())?;
        read_time += read_started.elapsed().as_secs_f64();
        let upload_started = Instant::now();
        let dst = unsafe {
            NonNull::new_unchecked(device.as_ptr().cast::<u8>().add(offset).cast::<c_void>())
        };
        unsafe {
            cuda::memcpy_async_host_to_device(dst, staging.as_ptr(), n, stream)
        }
        .map_err(|e| e.to_string())?;
        cuda::synchronize_stream(stream).map_err(|e| e.to_string())?;
        upload_time += upload_started.elapsed().as_secs_f64();
        offset += n;
    }
    let serial = started.elapsed().as_secs_f64();
    println!(
        "pipeline serial     {:.2} GB: {:>6.3} s  {:>5.2} GB/s (read {:.3} s + upload {:.3} s)",
        gb_of(bytes),
        serial,
        gb_of(bytes) / serial,
        read_time,
        upload_time
    );
    unsafe { cuda::free_host(staging) }.map_err(|e| e.to_string())?;

    // Overlapped: two staging blocks; while block N uploads, block N+1 is read.
    let a = unsafe { cuda::host_alloc_pinned(chunk) }.map_err(|e| e.to_string())?;
    let b = unsafe { cuda::host_alloc_pinned(chunk) }.map_err(|e| e.to_string())?;
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let started = Instant::now();
    let mut offset = 0usize;
    let mut slot = 0usize;
    let mut inflight: Option<usize> = None;
    while offset < bytes {
        let n = chunk.min(bytes - offset);
        let staging = if slot == 0 { a } else { b };
        let host =
            unsafe { std::slice::from_raw_parts_mut(staging.as_ptr().cast::<u8>(), n) };
        file.read_exact(host).map_err(|e| e.to_string())?;
        // Only now wait for the previous upload: it overlapped this read.
        if inflight.take().is_some() {
            cuda::synchronize_stream(stream).map_err(|e| e.to_string())?;
        }
        let dst = unsafe {
            NonNull::new_unchecked(device.as_ptr().cast::<u8>().add(offset).cast::<c_void>())
        };
        unsafe { cuda::memcpy_async_host_to_device(dst, staging.as_ptr(), n, stream) }
            .map_err(|e| e.to_string())?;
        inflight = Some(n);
        offset += n;
        slot ^= 1;
    }
    cuda::synchronize_stream(stream).map_err(|e| e.to_string())?;
    let overlapped = started.elapsed().as_secs_f64();
    println!(
        "pipeline overlapped {:.2} GB: {:>6.3} s  {:>5.2} GB/s",
        gb_of(bytes),
        overlapped,
        gb_of(bytes) / overlapped
    );

    unsafe { cuda::free_host(a) }.map_err(|e| e.to_string())?;
    unsafe { cuda::free_host(b) }.map_err(|e| e.to_string())?;
    unsafe { cuda::free(device) }.map_err(|e| e.to_string())?;
    cuda::destroy_stream(stream).map_err(|e| e.to_string())?;
    Ok(())
}

/// Teardown cost: N device buffers freed one by one (what dropping a weight
/// map does) plus a host arena drop, timed separately.
fn evict(size_gb: f64, count: usize) -> Result<(), String> {
    let bytes = (size_gb * GB as f64) as usize;
    let each = bytes / count.max(1);
    let stream = cuda::create_non_blocking_stream().map_err(|e| e.to_string())?;

    let started = Instant::now();
    let mut buffers = Vec::with_capacity(count);
    for _ in 0..count {
        buffers.push(unsafe { cuda::malloc(each) }.map_err(|e| e.to_string())?);
    }
    println!(
        "alloc {} x {:.1} MB ({:.2} GB): {:.3} s",
        count,
        each as f64 / MB as f64,
        gb_of(each * count),
        started.elapsed().as_secs_f64()
    );

    // A host arena the size of the model, faulted in like a real load.
    let started = Instant::now();
    let mut arena = vec![0u8; bytes];
    for offset in (0..bytes).step_by(4096) {
        arena[offset] = 1;
    }
    println!(
        "host arena {:.2} GB alloc+fault: {:.3} s",
        gb_of(bytes),
        started.elapsed().as_secs_f64()
    );

    let started = Instant::now();
    cuda::synchronize_stream(stream).map_err(|e| e.to_string())?;
    println!("stream sync before teardown: {:.3} s", started.elapsed().as_secs_f64());

    let started = Instant::now();
    for buffer in buffers.drain(..) {
        unsafe { cuda::free(buffer) }.map_err(|e| e.to_string())?;
    }
    let device_free = started.elapsed().as_secs_f64();
    println!(
        "device free {} buffers: {:.3} s ({:.1} us each)",
        count,
        device_free,
        device_free * 1e6 / count as f64
    );

    let started = Instant::now();
    drop(arena);
    println!("host arena drop: {:.3} s", started.elapsed().as_secs_f64());

    let started = Instant::now();
    cuda::device_synchronize().map_err(|e| e.to_string())?;
    println!("device synchronize after: {:.3} s", started.elapsed().as_secs_f64());

    cuda::destroy_stream(stream).map_err(|e| e.to_string())?;
    Ok(())
}

/// Page-locking is a kernel operation on every page: measure it, because a
/// swap that allocates fresh pinned staging every time pays it every time.
fn pinned(size_gb: f64) -> Result<(), String> {
    let bytes = (size_gb * GB as f64) as usize;
    for _ in 0..2 {
        let started = Instant::now();
        let buffer = unsafe { cuda::host_alloc_pinned(bytes) }.map_err(|e| e.to_string())?;
        let alloc = started.elapsed().as_secs_f64();
        let started = Instant::now();
        unsafe { cuda::free_host(buffer) }.map_err(|e| e.to_string())?;
        println!(
            "pinned {:.2} GB: alloc {:.3} s ({:.2} GB/s), free {:.3} s",
            gb_of(bytes),
            alloc,
            gb_of(bytes) / alloc,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
