//! Kernels run on the real device and match the CPU (skipped without a
//! Vulkan device). Run on node 165 and on the phone.
use makepad_ai_vulkan::*;
use std::sync::{Arc, Barrier};

fn setup() -> Option<(Arc<Device>, Library)> {
    let device = match Device::shared() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("no Vulkan device ({e}); skipped");
            return None;
        }
    };
    eprintln!("{}", device.features.description());
    let library = Library::new(KERNEL_SOURCES).expect("kernel library");
    Some((device, library))
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
    let retained = match Device::shared() {
        Ok(device) => Arc::downgrade(&device),
        Err(e) => {
            eprintln!("shared Vulkan device churn skipped: no usable device ({e})");
            return;
        }
    };
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
    let Some((device, mut library)) = setup() else { return };
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
    let Some((device, mut library)) = setup() else { return };
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

/// Dispatch latency and bandwidth on this device (cargo test -- --ignored
/// bench --nocapture).
#[test]
#[ignore]
fn bench_dispatch_and_bandwidth() {
    let Some((device, mut library)) = setup() else { return };
    let k = library.get(&device, "scale").unwrap();
    for (n, reps) in [(2560usize, 400u32), (64 << 20, 10)] {
        let a = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
        let b = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let args = k.args().buffer("src", a.address).buffer("dst", b.address).u32("n", n as u32).f32("scale", 1.5);
            s.dispatch(&args, [(n as u32).div_ceil(256).min(65535), 1, 1]);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        let t0 = std::time::Instant::now();
        s.submit_wait().unwrap();
        let wall = t0.elapsed().as_secs_f64() * 1e3;
        let gpu = s.timings().unwrap()[0].1;
        let per = gpu / reps as f64;
        let gbs = (n * 8) as f64 / (per * 1e-3) / 1e9;
        println!("n {n}: {reps} dispatches gpu {gpu:.2} ms ({per:.4} ms each, {gbs:.1} GB/s) wall {wall:.2} ms");
    }
}

/// GPU time of single LLM kernels at decode sizes (cargo test -- --ignored
/// kernels --nocapture).
#[test]
#[ignore]
fn bench_llm_kernels() {
    let Some((device, mut library)) = setup() else { return };
    let n = 9216usize;
    let a = device.buffer((n * 4 * 4) as u64, BufferKind::Device).unwrap();
    let b = device.buffer((n * 4 * 4) as u64, BufferKind::Device).unwrap();
    let c = device.buffer((n * 4 * 4) as u64, BufferKind::Device).unwrap();
    device.upload(&a, 0, &bytes(&vec![0.5f32; n * 4])).unwrap();
    device.upload(&b, 0, &bytes(&vec![0.25f32; n * 4])).unwrap();
    let reps = 200u32;
    let cases: Vec<(&str, Box<dyn Fn(&Kernel) -> (Args<'_>, [u32; 3])>)> = vec![
        ("scale", Box::new(|k: &Kernel| (k.args().buffer("src", a.address).buffer("dst", c.address).u32("n", n as u32).f32("scale", 2.0), [36, 1, 1]))),
        ("unary", Box::new(|k: &Kernel| (k.args().buffer("x", a.address).buffer("dst", c.address).u32("n", n as u32).u32("groups_x", 36).u32("op", 10), [36, 1, 1]))),
        ("glu", Box::new(|k: &Kernel| (k.args().buffer("a", a.address).buffer("b", b.address).buffer("dst", c.address).u32("n", n as u32).u32("groups_x", 36).u32("op", 2), [36, 1, 1]))),
        ("norm", Box::new(|k: &Kernel| (k.args().buffer("x", a.address).buffer("dst", c.address).u32("ne0", 2560).u32("ne1", 1).u32("ne2", 1)
            .u32("x_nb1", 2560).u32("x_nb2", 2560).u32("x_nb3", 2560).u32("d_nb1", 2560).u32("d_nb2", 2560).u32("d_nb3", 2560).f32("eps", 1e-6).u32("l2", 0), [1, 1, 1]))),
        ("binary", Box::new(|k: &Kernel| (k.args().buffer("a", a.address).buffer("b", b.address).buffer("dst", c.address).u32("op", 2)
            .u32("ne0", 2560).u32("ne1", 1).u32("ne2", 1).u32("b_ne0", 2560).u32("b_ne1", 1).u32("b_ne2", 1).u32("b_ne3", 1)
            .u32("a_nb0", 1).u32("a_nb1", 2560).u32("a_nb2", 2560).u32("a_nb3", 2560).u32("b_nb0", 1).u32("b_nb1", 2560).u32("b_nb2", 2560).u32("b_nb3", 2560)
            .u32("d_nb0", 1).u32("d_nb1", 2560).u32("d_nb2", 2560).u32("d_nb3", 2560), [10, 1, 1]))),
    ];
    // The pair the LLM graph runs after every norm: norm then mul.
    {
        let kn = library.get(&device, "norm").unwrap();
        let kb = library.get(&device, "binary").unwrap();
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let (a1, g1) = (cases[3].1)(&kn);
            s.dispatch(&a1, g1);
            s.barrier();
            let (a2, g2) = (cases[4].1)(&kb);
            s.dispatch(&a2, g2);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let gpu = s.timings().unwrap()[0].1;
        println!("norm+binary: {:.4} ms per pair", gpu / reps as f64);
    }
    for (name, make) in &cases {
        let k = library.get(&device, name).unwrap();
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let (args, groups) = make(&k);
            s.dispatch(&args, groups);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let gpu = s.timings().unwrap()[0].1;
        println!("{name}: {:.4} ms per dispatch", gpu / reps as f64);
    }
}

/// GPU time of the quantised gemv at the 4B model's FFN size.
#[test]
#[ignore]
fn bench_gemv() {
    let Some((device, mut library)) = setup() else { return };
    let (k, n) = (2560usize, 9216usize);
    for (name, unit) in [("gemv_q5k", 176usize), ("gemv_q6k", 224), ("gemv_q4k", 144), ("gemv_q80", 272)] {
        let row = k / 256 * unit;
        let mut w = vec![0u8; row * n];
        for (i, b) in w.iter_mut().enumerate() {
            *b = (i * 2654435761usize >> 13) as u8 & 0x3b;
        }
        let wb = device.buffer(w.len() as u64, BufferKind::Device).unwrap();
        device.upload(&wb, 0, &w).unwrap();
        let xq = device.buffer(k as u64, BufferKind::Device).unwrap();
        let xs = device.buffer((k / 32 * 8) as u64, BufferKind::Device).unwrap();
        let y = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
        let kern = library.get(&device, name).unwrap();
        let reps = 50u32;
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let args = kern.args().buffer("w", wb.address).buffer("w4", wb.address).buffer("xq", xq.address).buffer("xq4", xq.address).buffer("xs", xs.address).buffer("xs4", xs.address).buffer("dst", y.address)
                .u32("k", k as u32).u32("n", n as u32).u32("m", 1).u32("w_row", row as u32).u32("d_col", n as u32);
            s.dispatch(&args, [(n as u32).div_ceil(8), 1, 1]);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let ms = s.timings().unwrap()[0].1 / reps as f64;
        println!("{name}: {ms:.3} ms, {:.1} GB/s", w.len() as f64 / (ms * 1e-3) / 1e9);
    }
}

/// Where the gemv's time goes on this device: the same loop with parts
/// removed (cargo test -- --ignored probe --nocapture).
#[test]
#[ignore]
fn bench_gemv_probe() {
    let Some((device, _)) = setup() else { return };
    let probe = r#"
let base = {
    workgroup_size: 128
    w4: shader.buffer(vec4u)
    x4b: shader.buffer(vec4f)
    dst: shader.buffer_rw(f32)
    units: shader.uniform(u32)
    n: shader.uniform(u32)
    w_row16: shader.uniform(u32)
}
{
    // 16-byte weight loads only, summed as floats: the bandwidth ceiling.
    loads: base{
        main: fn() {
            let lane = local_id().x % 32
            let row0 = (group_id().x * 4 + local_id().x / 32) * 4
            var acc = 0.0
            for u in 0..self.units {
                for r in 0..4 {
                    let v = self.w4[(row0 + r) * self.w_row16 + u * 11 + (lane % 11)]
                    acc += float(v.x ^ v.y ^ v.z ^ v.w)
                }
            }
            if acc == 1.5 { self.dst[row0] = acc }
        }
    }
    // The same plus the 32-lane butterfly per row.
    loads_shuffle: base{
        main: fn() {
            let lane = local_id().x % 32
            let row0 = (group_id().x * 4 + local_id().x / 32) * 4
            var acc = array(0.0, 0.0, 0.0, 0.0)
            for u in 0..self.units {
                for r in 0..4 {
                    let v = self.w4[(row0 + r) * self.w_row16 + u * 11 + (lane % 11)]
                    acc[r] += float(v.x ^ v.y ^ v.z ^ v.w)
                }
            }
            for r in 0..4 {
                var s = acc[r]
                s += subgroup_shuffle_xor(s, 16)
                s += subgroup_shuffle_xor(s, 8)
                s += subgroup_shuffle_xor(s, 4)
                s += subgroup_shuffle_xor(s, 2)
                s += subgroup_shuffle_xor(s, 1)
                if lane == 0 { self.dst[row0 + r] = s }
            }
        }
    }
}
"#;
    let mut lib = Library::new(&[("probe.splash", probe)]).expect("probe kernels");
    let (units, n) = (10u32, 9216u32);
    let row16 = units * 11;
    let w = device.buffer((row16 * 16 * n) as u64, BufferKind::Device).unwrap();
    let x = device.buffer(65536, BufferKind::Device).unwrap();
    let y = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    for name in ["loads", "loads_shuffle"] {
        let k = lib.get(&device, name).unwrap();
        let reps = 50u32;
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let args = k.args().buffer("w4", w.address).buffer("x4b", x.address).buffer("dst", y.address).u32("units", units).u32("n", n).u32("w_row16", row16);
            s.dispatch(&args, [n / 16, 1, 1]);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let ms = s.timings().unwrap()[0].1 / reps as f64;
        println!("{name}: {ms:.3} ms, {:.1} GB/s", (row16 * 16 * n) as f64 / (ms * 1e-3) / 1e9);
    }
}

/// A decode-like sequence: gemv, then the small ops that follow it.
#[test]
#[ignore]
fn bench_sequence() {
    let Some((device, mut library)) = setup() else { return };
    let (k, n) = (2560usize, 9216usize);
    let row = k / 256 * 176;
    let wb = device.buffer((row * n) as u64, BufferKind::Device).unwrap();
    let xq = device.buffer(k as u64, BufferKind::Device).unwrap();
    let xs = device.buffer((k / 32 * 8) as u64, BufferKind::Device).unwrap();
    let a = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    let b = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    let c = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    let gemv = library.get(&device, "gemv_q5k").unwrap();
    let norm = library.get(&device, "norm").unwrap();
    let bin = library.get(&device, "binary").unwrap();
    let reps = 50u32;
    for with_small in [false, true] {
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let g = gemv.args().buffer("w4", wb.address).buffer("xq4", xq.address).buffer("xs", xs.address).buffer("xs4", xs.address).buffer("dst", a.address)
                .u32("k", k as u32).u32("n", n as u32).u32("m", 1).u32("w_row", row as u32).u32("d_col", n as u32);
            s.dispatch(&g, [(n as u32).div_ceil(16), 1, 1]);
            s.barrier();
            if with_small {
                for _ in 0..6 {
                    let nm = norm.args().buffer("x", a.address).buffer("dst", c.address).u32("ne0", 2560).u32("ne1", 1).u32("ne2", 1)
                        .u32("x_nb1", 2560).u32("x_nb2", 2560).u32("x_nb3", 2560).u32("d_nb1", 2560).u32("d_nb2", 2560).u32("d_nb3", 2560).f32("eps", 1e-6).u32("l2", 0);
                    s.dispatch(&nm, [1, 1, 1]);
                    s.barrier();
                    let bm = bin.args().buffer("a", c.address).buffer("b", b.address).buffer("dst", c.address).u32("op", 2)
                        .u32("ne0", 2560).u32("ne1", 1).u32("ne2", 1).u32("b_ne0", 2560).u32("b_ne1", 1).u32("b_ne2", 1).u32("b_ne3", 1)
                        .u32("a_nb0", 1).u32("a_nb1", 2560).u32("a_nb2", 2560).u32("a_nb3", 2560).u32("b_nb0", 1).u32("b_nb1", 2560).u32("b_nb2", 2560).u32("b_nb3", 2560)
                        .u32("d_nb0", 1).u32("d_nb1", 2560).u32("d_nb2", 2560).u32("d_nb3", 2560);
                    s.dispatch(&bm, [10, 1, 1]);
                    s.barrier();
                }
            }
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let ms = s.timings().unwrap()[0].1 / reps as f64;
        println!("gemv{}: {ms:.3} ms per step", if with_small { " + 12 small ops" } else { "" });
    }
}

/// The q5_K gemv's memory pattern with the decode math removed: what the
/// loads alone cost (cargo test -- --ignored pattern --nocapture).
#[test]
#[ignore]
fn bench_gemv_pattern() {
    let Some((device, _)) = setup() else { return };
    let src = format!("{}\n{{\n    loads_only: gemv_k{{\n        unit_bytes: 176\n        qs_at: 48\n        high_bit: 1\n        unit_part: fn(bo, part, x_lo, x_hi, sc) {{\n            let head = self.w4[bo >> 4]\n            let q = self.w4[(bo + self.qs_at) / 16 + part]\n            let h = self.w4[(bo + 16) / 16 + (part & 1)]\n            return float((head.x ^ q.x ^ q.y ^ q.z ^ q.w ^ h.x ^ h.w ^ x_lo.x ^ x_hi.y) & u32(255)) * sc.x\n        }}\n    }}\n    no_head: gemv_k{{\n        unit_bytes: 176\n        qs_at: 48\n        high_bit: 1\n        unit_part: fn(bo, part, x_lo, x_hi, sc) {{\n            let q = self.w4[(bo + self.qs_at) / 16 + part]\n            return float((q.x ^ q.y ^ q.z ^ q.w ^ x_lo.x ^ x_hi.y) & u32(255)) * sc.x\n        }}\n    }}\n}}\n", KERNEL_SOURCES[1].1.split("\n{\n    // ---- copies").next().unwrap());
    let mut lib = Library::new(&[("pattern.splash", &src)]).expect("pattern kernels");
    let (k, n) = (2560usize, 9216usize);
    let row = k / 256 * 176;
    let wb = device.buffer((row * n) as u64, BufferKind::Device).unwrap();
    let xq = device.buffer(k as u64, BufferKind::Device).unwrap();
    let xs = device.buffer((k / 32 * 8) as u64, BufferKind::Device).unwrap();
    let y = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    for name in ["loads_only", "no_head"] {
        let kern = lib.get(&device, name).unwrap();
        let reps = 50u32;
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let args = kern.args().buffer("w4", wb.address).buffer("xq4", xq.address).buffer("xs", xs.address).buffer("xs4", xs.address).buffer("dst", y.address)
                .u32("k", k as u32).u32("n", n as u32).u32("m", 1).u32("w_row", row as u32).u32("d_col", n as u32);
            s.dispatch(&args, [(n as u32).div_ceil(8), 1, 1]);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let ms = s.timings().unwrap()[0].1 / reps as f64;
        println!("{name}: {ms:.3} ms, {:.1} GB/s", (row * n) as f64 / (ms * 1e-3) / 1e9);
    }
}

/// Rows per lane group for the q5_K gemv (cargo test -- --ignored rpg --nocapture).
#[test]
#[ignore]
fn bench_gemv_rpg() {
    let Some((device, _)) = setup() else { return };
    let src = format!("{}\n{{\n    r1: gemv_k{{ unit_bytes: 176\n qs_at: 48\n high_bit: 1 }}\n    r2: gemv_k{{ unit_bytes: 176\n qs_at: 48\n high_bit: 1\n pair: 1 }}\n    r4: gemv_k{{ unit_bytes: 176\n qs_at: 48\n high_bit: 1\n pair: 1\n workgroup_size: 128 }}\n    r8: gemv_k{{ unit_bytes: 176\n qs_at: 48\n high_bit: 1\n pair: 1\n workgroup_size: 512 }}\n}}\n", KERNEL_SOURCES[1].1.split("\n{\n    // ---- copies").next().unwrap());
    let mut lib = Library::new(&[("rpg.splash", &src)]).expect("rpg kernels");
    let (k, n) = (2560usize, 9216usize);
    let row = k / 256 * 176;
    let wb = device.buffer((row * n) as u64, BufferKind::Device).unwrap();
    let xq = device.buffer(k as u64, BufferKind::Device).unwrap();
    let xs = device.buffer((k / 32 * 8) as u64, BufferKind::Device).unwrap();
    let y = device.buffer((n * 4) as u64, BufferKind::Device).unwrap();
    for (name, rows_per_wg) in [("r1", 8u32), ("r2", 8), ("r4", 4), ("r8", 16)] {
        let kern = lib.get(&device, name).unwrap();
        let reps = 50u32;
        let mut s = device.stream(2).unwrap();
        s.begin().unwrap();
        s.mark("start");
        for _ in 0..reps {
            let args = kern.args().buffer("w4", wb.address).buffer("xq4", xq.address).buffer("xs", xs.address).buffer("xs4", xs.address).buffer("dst", y.address)
                .u32("k", k as u32).u32("n", n as u32).u32("m", 1).u32("w_row", row as u32).u32("d_col", n as u32);
            s.dispatch(&args, [(n as u32).div_ceil(rows_per_wg), 1, 1]);
            s.barrier();
        }
        s.mark("end");
        s.submit_wait().unwrap();
        s.submit_wait().unwrap();
        let ms = s.timings().unwrap()[0].1 / reps as f64;
        println!("{name}: {ms:.3} ms, {:.1} GB/s", (row * n) as f64 / (ms * 1e-3) / 1e9);
    }
}

/// Creates the pipeline of every library kernel on this device: a driver
/// can reject SPIR-V that validates (cargo test -- --ignored pipelines
/// --nocapture).
#[test]
#[ignore]
fn pipelines_create() {
    let Some((device, _)) = setup() else { return };
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
