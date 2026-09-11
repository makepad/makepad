//! Cleanup DL-0 micro-probe: what a Metal buffer costs on this machine, as the
//! contract's "measured" section cites it. Cold allocation, first touch, a
//! CPU copy into shared storage, a warm re-allocation after a release, an
//! empty command-buffer round trip, a GPU blit of the same bytes, and the
//! release itself. Prints one line per size; asserts only sanity (non-null,
//! copied bytes readable back). Run with
//! `cargo test --release -p makepad-platform --test metal_alloc_probe -- --nocapture`.

#[cfg(target_os = "macos")]
fn main() {
    use makepad_platform::os::apple::apple_sys::*;
    use std::time::Instant;

    fn median(v: &mut Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[v.len() / 2]
    }

    unsafe {
        let pool: ObjcId = msg_send![class!(NSAutoreleasePool), new];
        let device = MTLCreateSystemDefaultDevice();
        assert!(!device.is_null(), "no Metal device");
        let queue: ObjcId = msg_send![device, newCommandQueue];
        assert!(!queue.is_null());
        let page = 16384usize;

        // Empty command buffer: the floor of one submission's commit + wait.
        let mut empty = Vec::new();
        for _ in 0..7 {
            let t = Instant::now();
            let cb: ObjcId = msg_send![queue, commandBuffer];
            let () = msg_send![cb, commit];
            let () = msg_send![cb, waitUntilCompleted];
            empty.push(t.elapsed().as_secs_f64() * 1e6);
        }
        println!("probe: empty_cb_commit_wait_us={:.0} (median of 7)", median(&mut empty));

        for &size in &[64usize << 10, 1 << 20, 4 << 20, 16 << 20] {
            let source: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let (mut cold, mut touch, mut copy, mut warm, mut blit, mut release) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for _ in 0..5 {
                let t = Instant::now();
                let buffer: ObjcId = msg_send![device, newBufferWithLength: size as u64 options: MTLResourceOptions::StorageModeShared];
                cold.push(t.elapsed().as_secs_f64() * 1e6);
                assert!(!buffer.is_null());
                let contents: *mut u8 = msg_send![buffer, contents];
                let t = Instant::now();
                let mut off = 0;
                while off < size {
                    contents.add(off).write_volatile(1);
                    off += page;
                }
                touch.push(t.elapsed().as_secs_f64() * 1e6);
                let t = Instant::now();
                std::ptr::copy_nonoverlapping(source.as_ptr(), contents, size);
                copy.push(t.elapsed().as_secs_f64() * 1e6);
                assert_eq!(*contents.add(size - 1), source[size - 1]);

                // GPU blit of the same bytes into a second buffer.
                let target: ObjcId = msg_send![device, newBufferWithLength: size as u64 options: MTLResourceOptions::StorageModeShared];
                let t = Instant::now();
                let cb: ObjcId = msg_send![queue, commandBuffer];
                let enc: ObjcId = msg_send![cb, blitCommandEncoder];
                let () = msg_send![enc, copyFromBuffer: buffer sourceOffset: 0u64 toBuffer: target destinationOffset: 0u64 size: size as u64];
                let () = msg_send![enc, endEncoding];
                let () = msg_send![cb, commit];
                let () = msg_send![cb, waitUntilCompleted];
                blit.push(t.elapsed().as_secs_f64() * 1e6);
                let tc: *const u8 = msg_send![target, contents];
                assert_eq!(*tc.add(size - 1), source[size - 1]);
                let () = msg_send![target, release];

                let t = Instant::now();
                let () = msg_send![buffer, release];
                release.push(t.elapsed().as_secs_f64() * 1e6);
                // Warm: the allocator has just seen a release of this size.
                let t = Instant::now();
                let again: ObjcId = msg_send![device, newBufferWithLength: size as u64 options: MTLResourceOptions::StorageModeShared];
                warm.push(t.elapsed().as_secs_f64() * 1e6);
                let () = msg_send![again, release];
            }
            println!(
                "probe: size={} cold_alloc_us={:.0} first_touch_us={:.0} memcpy_us={:.0} warm_alloc_us={:.0} gpu_blit_commit_wait_us={:.0} release_us={:.0} (medians of 5)",
                size, median(&mut cold), median(&mut touch), median(&mut copy), median(&mut warm), median(&mut blit), median(&mut release)
            );
        }
        let () = msg_send![queue, release];
        let () = msg_send![pool, release];
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
