#![cfg(not(target_arch = "wasm32"))]
use makepad_platform::{*, makepad_hal as hal, thread::Class};
use std::{sync::mpsc, time::{Duration, Instant}};

// Both fixtures use the process HAL; a device replacement must not interrupt its peer.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn grouped_gpu_reads_are_pool_leases_without_cpu_copies() {
    let _gpu = GPU.lock().unwrap();
    let backend = if cfg!(target_vendor = "apple") { hal::backend_id::METAL }
        else if cfg!(windows) { hal::backend_id::D3D11 } else { hal::backend_id::VULKAN };
    let info = hal::hal_open(&hal::OpenReq { backend, flags: hal::open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend, "requires the native GPU backend");
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.hal_attach_session(info);
    let buffer = GpuBuffer::new(&mut cx, 128, Default::default()).unwrap();
    let kernel = cx.with_vm(|vm| {
        vm.capture_errors();
        let value = vm.eval(makepad_script::ScriptMod::new("", "", "lease_write", 0, 0,
            "use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{workgroup_size: 32\nwords: shader.buffer_rw(u32)\nmain: fn() { let i = global_id().x\nself.words[i] = i * 7u + 3u }}", &[]));
        let kernel = ComputeKernel::new(vm, value.as_object().unwrap(), "lease_write").unwrap();
        assert!(vm.take_errors().is_empty());
        kernel
    });
    let mut pass = ComputePass::new("lease writes", None);
    // Exactly 32 invocations, one store each, no data-dependent loop.
    pass.dispatch(&kernel, [1, 1, 1], &kernel.params(), &[buffer.clone()], &[]).unwrap();
    pass.submit(&mut cx);
    let copies = hal::lease::readback_copy_count();
    let ui = std::thread::current().id();
    let (tx, rx) = mpsc::channel();
    for request in 0..3 {
        let tx = tx.clone();
        cx.read_to_pool(&[(&buffer, 64..128), (&buffer, 0..64), (&buffer, 3..11), (&buffer, 127..128)], Class::Interactive, move |result| {
            tx.send((request, std::thread::current().id(), result)).unwrap();
        }).unwrap();
    }
    let start = Instant::now();
    let mut received = [false; 3];
    let mut saved = None;
    while received.iter().any(|b| !b) {
        assert!(start.elapsed() < Duration::from_secs(10), "grouped read completion deadline");
        cx.hal_session_turn(1_000_000, true);
        while let Ok((request, thread, result)) = rx.try_recv() {
            assert!(!received[request], "a request answered twice");
            received[request] = true;
            assert_ne!(thread, ui, "heavy callback ran on UI");
            let ranges = result.unwrap();
            assert_eq!(ranges.len(), 4);
            for range in &ranges {
                assert!(format!("{range:?}").starts_with("HostBuf::Lease"));
                if !cfg!(windows) {
                    assert!(hal::lease::hal_lease_in_block(range.as_ptr() as u64, range.len() as u64), "bytes must belong to a mapped HAL lease block");
                }
            }
            for (range, first) in ranges.iter().zip([16u32, 0]) {
                let words: Vec<_> = range.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
                assert_eq!(words, (first..first + 16).map(|i| i * 7 + 3).collect::<Vec<_>>(), "GPU-written bytes in request order");
            }
            let expected: Vec<_> = (0u32..32).flat_map(|i| (i * 7 + 3).to_le_bytes()).collect();
            assert_eq!(&*ranges[2], &expected[3..11], "unaligned range");
            assert_eq!(&*ranges[3], &expected[127..128], "last byte range");
            saved = Some(ranges);
        }
    }
    assert_eq!(hal::lease::readback_copy_count(), copies, "HAL and Cx copied no readback payload bytes");
    cx.hal_close_session();
    let ranges = saved.unwrap();
    assert_eq!(&ranges[1][..4], &3u32.to_le_bytes(), "session shutdown must not revoke a delivered lease");
    drop(ranges);
}

#[test]
fn device_loss_answers_pending_reads_and_keeps_native_lease_readable() {
    let _gpu = GPU.lock().unwrap();
    let backend = if cfg!(target_vendor = "apple") { hal::backend_id::METAL }
        else if cfg!(windows) { hal::backend_id::D3D11 } else { hal::backend_id::VULKAN };
    let info = hal::hal_open(&hal::OpenReq { backend, flags: hal::open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend);
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.hal_attach_session(info);
    let buffer = GpuBuffer::new(&mut cx, 16, Default::default()).unwrap();
    buffer.update(&mut cx, 0, &[7; 16]).unwrap();
    let (tx, rx) = mpsc::channel();
    cx.read_to_pool(&[(&buffer, 0..16)], Class::Interactive, move |r| { tx.send(r).unwrap(); }).unwrap();
    let start = Instant::now();
    let kept = loop {
        if let Ok(result) = rx.try_recv() { break result.unwrap().remove(0); }
        assert!(start.elapsed() < Duration::from_secs(10), "initial read deadline");
        cx.hal_session_turn(1_000_000, true);
    };
    let (tx, rx) = mpsc::channel();
    for i in 0..4 {
        let tx = tx.clone();
        let ranges = if i % 2 == 0 { vec![(&buffer, 0..4), (&buffer, 8..16)] } else { vec![(&buffer, 4..8)] };
        if i < 2 { cx.read_to_pool(&ranges, Class::Interactive, move |r| { tx.send((i, r)).unwrap(); }).unwrap(); }
        else { cx.read(&ranges, move |_, r| { tx.send((i, r)).unwrap(); }).unwrap(); }
    }
    hal::Hal::global().simulate_device_loss();
    let start = Instant::now();
    let mut answers = [0; 4];
    while answers.contains(&0) {
        assert!(start.elapsed() < Duration::from_secs(10), "every lost read must answer");
        cx.hal_session_turn(1_000_000, true);
        while let Ok((i, result)) = rx.try_recv() {
            assert_eq!(result.unwrap_err(), ReadbackError::DeviceLost);
            answers[i] += 1;
        }
    }
    assert_eq!(answers, [1; 4]);
    assert_eq!(&*kept, &[7; 16], "old-device mapped storage stays readable");
    cx.hal_close_session();
    assert_eq!(&*kept, &[7; 16], "closing the session also retains the lease");
    drop(kept);
    assert!(rx.try_recv().is_err(), "no duplicate completion after shutdown");
}
