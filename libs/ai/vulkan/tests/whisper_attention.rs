//! Real-device encoder attention checks. The ignored benchmark is also a
//! standalone test binary: --ignored --nocapture --test-threads=1.
use makepad_ai_vulkan::*;

fn data(len: usize, mut seed: u32, gain: f32) -> Vec<f32> {
    (0..len)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            ((seed & 65535) as f32 / 32768.0 - 1.0) * gain
        })
        .collect()
}

fn bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

// Full f64 reference, including every output in the large benchmark.
// Contiguous inner loops keep it practical on the phone as well as the host.
fn reference(q: &[f32], k: &[f32], v: &[f32], nq: usize, nk: usize, heads: usize) -> Vec<f32> {
    let width = heads * 64;
    let mut out = vec![0.0; nq * width];
    let mut kt = vec![0.0f64; 64 * nk];
    let mut vh = vec![0.0f64; nk * 64];
    let mut scores = vec![0.0f64; nk];
    for h in 0..heads {
        for j in 0..nk {
            for d in 0..64 {
                kt[d * nk + j] = k[j * width + h * 64 + d] as f64;
                vh[j * 64 + d] = v[j * width + h * 64 + d] as f64;
            }
        }
        for i in 0..nq {
            scores.fill(0.0);
            for d in 0..64 {
                let a = q[i * width + h * 64 + d] as f64 * 0.125;
                for j in 0..nk {
                    scores[j] += a * kt[d * nk + j];
                }
            }
            let mx = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let mut sum = 0.0;
            for score in &mut scores {
                *score = (*score - mx).exp();
                sum += *score;
            }
            let mut row = [0.0f64; 64];
            for j in 0..nk {
                let p = scores[j] / sum;
                for d in 0..64 {
                    row[d] += p * vh[j * 64 + d];
                }
            }
            for d in 0..64 {
                out[i * width + h * 64 + d] = row[d] as f32;
            }
        }
    }
    out
}

fn run(
    device: &Device,
    library: &mut Library,
    nq: usize,
    nk: usize,
    heads: usize,
    gain: f32,
    bench: bool,
) {
    let q = data(nq * heads * 64, 0x13579, gain);
    let k = data(nk * heads * 64, 0x24681, gain);
    let v = data(nk * heads * 64, 0x75319, 1.0);
    if bench {
        eprintln!("attention {nq}x{nk} heads={heads} d=64: computing full CPU reference");
    }
    let want = reference(&q, &k, &v, nq, nk, heads);
    let upload = |values: &[f32]| {
        let b = device.buffer((values.len() * 4) as u64, BufferKind::Device).unwrap();
        device.upload(&b, 0, &bytes(values)).unwrap();
        b
    };
    let qb = upload(&q);
    let kb = upload(&k);
    let vb = upload(&v);
    let size = want.len() * 4;
    let dst = device.buffer((size + 128) as u64, BufferKind::Device).unwrap();
    for (name, tile) in [("whisper_attention", 8), ("attention_d64", 64)] {
        // Guards on both sides catch writes from padded query rows.
        device.upload(&dst, 0, &vec![0x5a; size + 128]).unwrap();
        let kernel = library.get(device, name).unwrap();
        let mut args = kernel.args().buffer("q", qb.address).buffer("k", kb.address)
            .buffer("v", vb.address).buffer("dst", dst.address + 64)
            .u32("nq", nq as u32).u32("nk", nk as u32).u32("heads", heads as u32)
            .f32("scale", 0.125);
        if tile == 8 {
            args = args.u32("causal", 0).u32("past", 0);
        } else {
            let row = (heads * 16) as u32;
            args = args.u32("q_row", row).u32("q_head", 16).u32("k_row", row).u32("k_head", 16)
                .u32("v_row", row).u32("v_head", 16).u32("d_row", row).u32("d_head", 16);
        }
        let mut stream = device.stream(2).unwrap();
        stream.begin().unwrap();
        stream.barrier();
        stream.mark("attention");
        stream.dispatch(&args, [heads as u32, nq.div_ceil(tile) as u32, 1]);
        stream.mark("end");
        stream.submit_wait().unwrap();
        // Download through cached staging memory on discrete devices.
        let staging = device.buffer((size + 128) as u64, BufferKind::Staging).unwrap();
        let mut copy = device.stream(0).unwrap();
        copy.begin().unwrap();
        copy.barrier();
        copy.copy(&dst, 0, &staging, 0, (size + 128) as u64);
        copy.submit_wait().unwrap();
        let got = device.download(&staging, 0, size + 128).unwrap();
        assert_eq!(&got[..64], &[0x5a; 64], "{name}: prefix guard");
        assert_eq!(&got[size + 64..], &[0x5a; 64], "{name}: suffix guard");
        let mut max_abs = 0.0f32;
        for (i, (value, expected)) in got[64..size + 64].chunks_exact(4).zip(&want).enumerate() {
            let value = f32::from_le_bytes(value.try_into().unwrap());
            let error = (value - expected).abs();
            max_abs = max_abs.max(error);
            let tolerance = if gain > 1.0 { 2e-4 } else { 2e-5 };
            assert!(value.is_finite() && error <= tolerance,
                "{name} {nq}x{nk} h={heads} gain={gain} at {i}: {value} expected {expected}, error={error}");
        }
        if bench {
            for _ in 0..3 { stream.submit_wait().unwrap(); }
            let mut times = Vec::new();
            for _ in 0..11 {
                stream.submit_wait().unwrap();
                times.push(stream.timings().unwrap()[0].1);
            }
            times.sort_by(f64::total_cmp);
            let ms = times[times.len() / 2];
            let flops = 4.0 * nq as f64 * nk as f64 * heads as f64 * 64.0;
            println!("{name} nq={nq} nk={nk} heads={heads} d=64 median={ms:.4} ms effective={:.2} GFLOPS max_abs={max_abs:.8}", flops / (ms * 1e6));
        } else {
            eprintln!("{name} {nq}x{nk} heads={heads} gain={gain} max_abs={max_abs:.8}");
        }
    }
}

#[test]
fn whisper_attention_matches_cpu() {
    let device = Device::shared().expect("real Vulkan device");
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for (nq, nk, heads) in [(1, 1, 1), (7, 31, 2), (31, 7, 3), (63, 33, 2),
        (65, 65, 3), (67, 97, 2), (129, 129, 20), (600, 600, 1), (1500, 1500, 1)] {
        run(&device, &mut library, nq, nk, heads, 1.0, false);
    }
    // Sharp logits exercise the online rescaling when a later tile wins.
    run(&device, &mut library, 65, 97, 3, 8.0, false);
    // Equal scores exercise a zero query and uniform probabilities at tails.
    run(&device, &mut library, 67, 35, 2, 0.0, false);
}

#[test]
#[ignore = "GPU benchmark; run with --ignored --nocapture --test-threads=1"]
fn whisper_attention_bench() {
    let device = Device::shared().expect("real Vulkan device");
    eprintln!("{}", device.features.description());
    eprintln!("One dispatch includes all 20 heads. GFLOPS counts QK and PV only; no quadratic scratch allocation.");
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for rows in [1500, 600] {
        run(&device, &mut library, rows, rows, 20, 1.0, true);
    }
}
