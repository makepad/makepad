//! Kernels run on a required real Vulkan device and match the CPU.
//! Run on a pool node and on the phone.
use makepad_ai_vulkan::*;
use std::sync::{Arc, Barrier};

fn setup() -> (Arc<Device>, Library) {
    let device = Device::shared().expect("a working Vulkan device is required");
    eprintln!("{}", device.features.description());
    let library = Library::new(KERNEL_SOURCES).expect("kernel library");
    (device, library)
}

fn f32s(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

#[test]
fn shared_device_survives_100_worker_threads() {
    // Retain no caller's Arc between workers: only the process-wide owner
    // keeps the device alive. Its identity must survive all thread exits.
    let retained = Arc::downgrade(&Device::shared().expect("a working Vulkan device is required"));
    let identity = retained.as_ptr() as usize;
    fn exercise(identity: usize, seed: u32, rounds: u32) {
        let device = Device::shared().expect("shared device survives worker teardown");
        assert_eq!(Arc::as_ptr(&device) as usize, identity);
        let buffer = device.buffer(16, BufferKind::Device).unwrap();
        let staging = device.buffer(32, BufferKind::Staging).unwrap();
        let mut stream = device.stream(0).unwrap();
        for round in 0..rounds {
            let input = (seed + round).to_le_bytes().repeat(4);
            device.upload(&staging, 0, &input).unwrap();
            stream.begin().unwrap();
            stream.copy(&staging, 0, &buffer, 0, 16);
            stream.barrier();
            stream.copy(&buffer, 0, &staging, 16, 16);
            stream.submit_wait().unwrap();
            assert_eq!(device.download(&staging, 16, 16).unwrap(), input);
        }
    }
    for i in 0..100 {
        std::thread::spawn(move || exercise(identity, i, 1))
            .join().unwrap_or_else(|_| panic!("Vulkan worker {i} panicked"));
        assert!(retained.upgrade().is_some(), "device dropped with worker {i}");
    }
    // No test-wide GPU lock: independent streams must be safe to allocate,
    // record, submit, reset and destroy concurrently on the same device.
    let start = Arc::new(Barrier::new(2));
    let workers: Vec<_> = (0..2).map(|i| {
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            exercise(identity, 1000 + i * 100, 8);
        })
    }).collect();
    for worker in workers { worker.join().expect("concurrent Vulkan worker"); }
}

#[test]
fn every_kernel_compiles() {
    let mut library = Library::new(KERNEL_SOURCES).expect("kernel library");
    let mut failed = Vec::new();
    for name in library.names() {
        match library.compile(&name) {
            Err(e) => failed.push(e),
            Ok(k) => {
                if let Some(Err(e)) = makepad_script::shader_ir_spirv::spirv_val(&k.spirv) {
                    failed.push(format!("{name}: spirv-val: {e}"));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

#[test]
fn scale_matches_cpu() {
    let (device, mut library) = setup();
    let n = 100_003usize;
    let src: Vec<f32> = (0..n).map(|i| (i as f32).sin()).collect();
    let a = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    let b = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    device.upload(&a, 0, &bytes(&src)).unwrap();
    let k = library.get(&device, "scale").unwrap();
    let mut s = device.stream(0).unwrap();
    s.begin().unwrap();
    let args = k.args().buffer("src", a.address).buffer("dst", b.address).u32("n", n as u32).f32("scale", 2.5);
    s.dispatch(&args, [(n as u32).div_ceil(k.workgroup()[0]), 1, 1]);
    s.submit_wait().unwrap();
    let out = f32s(&device.download(&b, 0, n * 4).unwrap());
    for i in 0..n {
        assert_eq!(out[i], src[i] * 2.5, "at {i}");
    }
}

#[test]
fn row_sum_matches_cpu() {
    let (device, mut library) = setup();
    let (rows, cols) = (37usize, 4099usize);
    let x: Vec<f32> = (0..rows * cols).map(|i| ((i * 7919) % 1000) as f32 / 1000.0 - 0.5).collect();
    let xb = device.buffer((x.len() * 4) as u64, BufferKind::Device).unwrap();
    let yb = device.buffer((rows * 4) as u64, BufferKind::Device).unwrap();
    device.upload(&xb, 0, &bytes(&x)).unwrap();
    let k = library.get(&device, "row_sum").unwrap();
    let mut s = device.stream(0).unwrap();
    s.begin().unwrap();
    s.dispatch(&k.args().buffer("x", xb.address).buffer("y", yb.address).u32("cols", cols as u32), [rows as u32, 1, 1]);
    s.submit_wait().unwrap();
    let y = f32s(&device.download(&yb, 0, rows * 4).unwrap());
    for r in 0..rows {
        let want: f64 = x[r * cols..(r + 1) * cols].iter().map(|v| *v as f64).sum();
        assert!((y[r] as f64 - want).abs() < 1e-3, "row {r}: {} vs {want}", y[r]);
    }
}

/// Decode GEMV sweep: all opt-ins against the unchanged kernel, using
/// initialized weights and quantized activations. Run the release test
/// binary with `bench_gemv --exact --nocapture --test-threads=1`.
/// GB/s counts repacked weight bytes * m (each column reads the weights).
#[test]
fn bench_gemv() {
    let (device, mut library) = setup();
    let variants = [
        // Selector, lanes per row, rows per lane group, workgroup size.
        ("default", 32u32, 1u32, 256u32),
        ("word", 32, 1, 256),
        ("rows2", 32, 2, 256),
        ("rows4", 32, 4, 256),
        ("wide", 32, 1, 512),
        ("lanes64", 64, 1, 256),
        ("lanes128", 128, 1, 256),
    ];
    for (name, unit) in [("gemv_q5k", 176usize), ("gemv_q6k", 224), ("gemv_q4k", 144), ("gemv_q80", 272)] {
        // Short K, partial row groups and m=2/3 also exercise inactive
        // lanes, shared reduction barriers, and output column strides.
        let mut shapes = vec![(37usize, 256usize), (33, 768), (65, 1024)];
        if unit == 224 {
            // The lanes128 variant has two rows per group: cross 65,535 groups.
            shapes.push((131_071, 256));
        }
        for (n, k) in shapes {
            let row = k / 256 * unit;
            let mut w = vec![0u8; row * n];
            let mut random = 0x12345678u32;
            for block in w.chunks_exact_mut(unit) {
                for b in block.iter_mut() {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    *b = random as u8;
                }
                // Keep f16 scales finite, with varied signs/magnitudes;
                // all code, packed scale and high-bit bytes stay varied.
                let half = |salt: u32| (0x1400u16 + ((random ^ salt) as u16 & 0x03ff)) | (((random ^ salt) as u16 & 1) << 15);
                match unit {
                    144 | 176 => {
                        block[0..2].copy_from_slice(&half(0).to_le_bytes());
                        block[2..4].copy_from_slice(&half(37).to_le_bytes());
                    }
                    224 => block[208..210].copy_from_slice(&half(0).to_le_bytes()),
                    272 => {
                        for j in 0..8 {
                            block[j * 2..j * 2 + 2].copy_from_slice(&half(j as u32 * 73).to_le_bytes());
                        }
                    }
                    _ => unreachable!(),
                }
            }
            let wb = device.buffer(w.len() as u64, BufferKind::Device).unwrap();
            device.upload(&wb, 0, &w).unwrap();
            drop(w);
            let columns: &[usize] = if n < 100 { &[1, 2, 3, 4] } else { &[1, 4] };
            for &m in columns {
                let x: Vec<f32> = (0..k * m).map(|i| {
                    if (i / 32) % 11 == 0 { 0.0 } else {
                        (((i * 7919 + i / 7) % 1009) as f32 - 504.0) / 257.0
                    }
                }).collect();
                let xb = device.buffer((x.len() * 4) as u64, BufferKind::Device).unwrap();
                device.upload(&xb, 0, &bytes(&x)).unwrap();
                let xq = device.buffer((k * m) as u64, BufferKind::Device).unwrap();
                let xs = device.buffer((k * m / 32 * 8) as u64, BufferKind::Device).unwrap();
                let d_col = n + 3;
                let sentinel = bytes(&vec![f32::NAN; d_col * m]);
                let y = device.buffer(sentinel.len() as u64, BufferKind::Device).unwrap();
                let quant = library.get(&device, "quantize_q8").unwrap();
                let mut prepare = device.stream(0).unwrap();
                prepare.begin().unwrap();
                prepare.dispatch(&quant.args().buffer("x", xb.address).buffer("xq", xq.address).buffer("xs", xs.address)
                    .u32("blocks", (k * m / 32) as u32).u32("k", k as u32).u32("x_col", k as u32),
                    [((k * m / 32) as u32).div_ceil(64), 1, 1]);
                prepare.submit_wait().unwrap();
                let mut reference = Vec::<f32>::new();
                for (variant, lanes, rows, workgroup) in variants {
                    let f = &device.features;
                    assert!(workgroup <= f.max_workgroup_invocations.min(f.max_workgroup_size[0]), "{variant}: workgroup unsupported");
                    let subgroup = (lanes > 32 && f.subgroup_size_control
                        && (f.min_subgroup_size..=f.max_subgroup_size).contains(&lanes)).then_some(lanes);
                    let kernel_name = if variant == "default" { name.to_string() } else { format!("{name}_{variant}") };
                    let kern = library.get_constants_subgroup(&device, &kernel_name, &[], subgroup).unwrap();
                    assert_eq!(kern.workgroup()[0], workgroup);
                    let groups = (n as u32).div_ceil(workgroup / lanes * rows);
                    let groups_y = groups.div_ceil(65535).max(1);
                    let groups_x = groups.div_ceil(groups_y).max(1);
                    device.upload(&y, 0, &sentinel).unwrap();
                    let args = kern.args().buffer("w", wb.address).buffer("w4", wb.address).buffer("xq", xq.address).buffer("xq4", xq.address)
                        .buffer("xs", xs.address).buffer("xs4", xs.address).buffer("dst", y.address)
                        .u32("k", k as u32).u32("n", n as u32).u32("m", m as u32).u32("w_row", row as u32)
                        .u32("d_col", d_col as u32).u32("groups_x", groups_x);
                    let reps = 2;
                    let mut s = device.stream(2).unwrap();
                    s.begin().unwrap();
                    s.mark("start");
                    for _ in 0..reps {
                        s.dispatch(&args, [groups_x, groups_y, 1]);
                        s.barrier();
                    }
                    s.mark("end");
                    s.submit_wait().unwrap(); // Warm pipelines and caches.
                    let start = std::time::Instant::now();
                    s.submit_wait().unwrap();
                    let wall_ms = start.elapsed().as_secs_f64() * 1e3 / reps as f64;
                    let ms = s.timings().unwrap()[0].1 / reps as f64;
                    let out = f32s(&device.download(&y, 0, sentinel.len()).unwrap());
                    let mut error = 0.0f32;
                    let mut magnitude = 0.0f32;
                    let mut bit_differences = 0usize;
                    for col in 0..m {
                        for r in 0..n {
                            let i = col * d_col + r;
                            assert!(out[i].is_finite(), "{kernel_name} [{n}x{k}] m={m} col={col} row={r}: nonfinite or unwritten");
                            if !reference.is_empty() {
                                error = error.max((out[i] - reference[i]).abs());
                                magnitude = magnitude.max(reference[i].abs());
                                bit_differences += usize::from(out[i].to_bits() != reference[i].to_bits());
                            }
                        }
                        assert!(out[col * d_col + n..(col + 1) * d_col].iter().all(|v| v.is_nan()), "{kernel_name}: wrote column padding");
                    }
                    if reference.is_empty() {
                        reference = out;
                    }
                    let gbs = (row * n * m) as f64 / (ms * 1e6);
                    println!("{name} {variant:8} [{n}x{k}] m={m}: {ms:.4} ms GPU, {wall_ms:.4} ms wall, {gbs:.2} GB/s, max_error={error:.6e}, subgroup={} row_lanes={lanes}, bit_differences={bit_differences}", subgroup.unwrap_or(f.subgroup_size));
                    assert!(error <= 1e-4 * (1.0 + magnitude), "{kernel_name}: max error {error} at reference magnitude {magnitude}");
                }
            }
        }
    }
}

/// Creates the pipeline of every library kernel on this device: a driver
/// can reject SPIR-V that validates (cargo test -- pipelines
/// --nocapture).
#[test]
fn pipelines_create() {
    let (device, _) = setup();
    let mut library = Library::new(KERNEL_SOURCES).expect("kernel library");
    let mut failed = 0;
    for name in library.names() {
        if let Err(e) = library.get(&device, &name) {
            println!("PIPELINE_FAIL {name}: {e}");
            failed += 1;
        }
    }
    println!("pipelines: {failed} failed");
    assert_eq!(failed, 0);
}
