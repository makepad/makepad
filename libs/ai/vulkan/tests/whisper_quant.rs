//! Real-device block/tail checks and a standalone phone benchmark:
//! whisper_quant-* --ignored --nocapture --test-threads=1
use makepad_ai_vulkan::*;
use makepad_script::makepad_math::{f16_bits_to_f32, f32_to_f16_bits};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
enum Format { F16, Quant(BlockQuant) }

impl Format {
    fn stride(self, k: usize) -> usize {
        match self {
            Self::F16 => k.div_ceil(8) * 16,
            Self::Quant(q) => k.div_ceil(q.block_elements() as usize) * q.block_bytes() as usize,
        }
    }

    fn weights(self, n: usize, k: usize) -> Vec<u8> {
        let stride = self.stride(k);
        let mut bytes = vec![0; n * stride];
        match self {
            Self::F16 => {
                for r in 0..n {
                    for d in 0..k {
                        let at = r * stride + d * 2;
                        bytes[at..at + 2].copy_from_slice(&f32_to_f16_bits(value(r * k + d)).to_le_bytes());
                    }
                }
            }
            Self::Quant(q) => {
                let size = q.block_bytes() as usize;
                for (i, block) in bytes[..n * stride].chunks_exact_mut(size).enumerate() {
                    // Exercise every code, signed and zero scales, and
                    // nonzero padding in partial rows. Adjacent blocks have
                    // distinct metadata and alternate word alignment.
                    for (j, byte) in block.iter_mut().enumerate() {
                        *byte = (i * 17 + j * 59) as u8;
                    }
                    let scale = match i % 7 { 0 => 0.0, 1 => -0.007, 2 => 0.00001, _ => 0.023 };
                    block[..2].copy_from_slice(&f32_to_f16_bits(scale).to_le_bytes());
                    if q != BlockQuant::Q8_0 {
                        block[2..4].copy_from_slice(&f32_to_f16_bits(-0.17 + (i % 3) as f32 * 0.2).to_le_bytes());
                    }
                }
            }
        }
        bytes
    }

    // Scalar byte decoder independent of the GPU's unaligned word loads.
    fn weight(self, bytes: &[u8], k: usize, r: usize, d: usize) -> f32 {
        let row = &bytes[r * self.stride(k)..];
        let half = |b: &[u8]| f16_bits_to_f32(u16::from_le_bytes([b[0], b[1]]));
        match self {
            Self::F16 => half(&row[d * 2..]),
            Self::Quant(q) => {
                let b = &row[d / q.block_elements() as usize * q.block_bytes() as usize..];
                if q == BlockQuant::Q8_0 { return half(b) * b[2 + d % 32] as i8 as f32; }
                let i = d % 256;
                let j = i / 32;
                let (sc, mn) = if j < 4 { (b[4 + j] & 63, b[8 + j] & 63) }
                    else { ((b[8 + j] & 15) | ((b[j] >> 6) << 4),
                        (b[8 + j] >> 4) | ((b[4 + j] >> 6) << 4)) };
                let codes = if q == BlockQuant::Q5K { 48 } else { 16 };
                let lo = (b[codes + (i / 64) * 32 + i % 32] >> ((j % 2) * 4)) & 15;
                let hi = if q == BlockQuant::Q5K { ((b[16 + i % 32] >> j) & 1) * 16 } else { 0 };
                half(b) * sc as f32 * (lo + hi) as f32 - half(&b[2..]) * mn as f32
            }
        }
    }
}

fn value(i: usize) -> f32 { ((i * 7919 % 1009) as f32 - 504.0) / 509.0 }
fn bytes(v: &[f32]) -> Vec<u8> { v.iter().flat_map(|v| v.to_le_bytes()).collect() }

fn run(device: &Device, library: &mut Library, format: Format, [n, k, m]: [u32; 3], bias: bool, benchmark: bool) {
    let (nn, kk, mm) = (n as usize, k as usize, m as usize);
    let weights = format.weights(nn, kk);
    let x: Vec<_> = (0..mm * kk).map(|i| if (i / kk + (i % kk) / 32) % 11 == 7 { 0.0 } else { value(i) }).collect();
    // Independent CPU implementation of the shared activation quantiser,
    // including the final partial block of each row.
    let mut reference_x = x.clone();
    if matches!(format, Format::Quant(_)) {
        for row in reference_x.chunks_mut(kk) {
            for block in row.chunks_mut(32) {
                let d = block.iter().map(|v| v.abs()).fold(0.0f32, f32::max) / 127.0;
                let inv = if d == 0.0 { 0.0 } else { 1.0 / d };
                for v in block { *v = (*v * inv).round() * d; }
            }
        }
    }
    let b: Vec<_> = (0..nn).map(|i| value(i + 7)).collect();
    let packed = match format {
        Format::Quant(q) => q.pack(&weights, n, k).unwrap(),
        Format::F16 => std::borrow::Cow::Borrowed(weights.as_slice()),
    };
    let wb = device.buffer(packed.len() as u64, BufferKind::Device).unwrap();
    let xb = device.buffer((x.len() * 4) as u64, BufferKind::Device).unwrap();
    let bb = device.buffer((b.len() * 4) as u64, BufferKind::Device).unwrap();
    let dst = device.buffer((nn * mm * 4 + 64) as u64, BufferKind::Device).unwrap();
    device.upload(&wb, 0, &packed).unwrap();
    device.upload(&xb, 0, &bytes(&x)).unwrap();
    device.upload(&bb, 0, &bytes(&b)).unwrap();
    device.upload(&dst, (nn * mm * 4) as u64, &[0x5a; 64]).unwrap();
    // Retain scratch through submission and every timed replay.
    let matmul = match format {
        Format::Quant(q) => Some(QuantizedMatmul::new(device, library, n, k, m, q).unwrap()),
        Format::F16 => None,
    };
    let scratch = matmul.as_ref().map(|g| device.buffer(g.scratch_bytes(), BufferKind::Device).unwrap());
    let mut stream = device.stream(2).unwrap();
    stream.begin().unwrap();
    stream.mark("matmul");
    match format {
        Format::Quant(_) => {
            let matmul = matmul.as_ref().unwrap();
            assert_eq!(matmul.kernel.workgroup(), [if m <= 4 { 256 } else { 128 }, 1, 1]);
            matmul.encode(&mut stream, &wb, &xb, &dst, bias.then_some(&bb), scratch.as_ref().unwrap()).unwrap();
        }
        Format::F16 if m > 4 => {
            Gemm::new_nt(device, library, n, k, m, ActivationType::F16, (format.stride(kk) / 2) as u32)
                .unwrap().encode_bias(&mut stream, &wb, &xb, &dst, bias.then_some(&bb)).unwrap();
        }
        Format::F16 => {
            let kernel = library.get(device, "whisper_gemv_f16").unwrap();
            let args = kernel.args().buffer("x", xb.address).buffer("x4", xb.address)
                .buffer("w4", wb.address).buffer("dst", dst.address).buffer("bias", bb.address)
                .u32("m", m).u32("n", n).u32("k", k).u32("ws", (format.stride(kk) / 2) as u32)
                .u32("has_bias", u32::from(bias));
            stream.dispatch(&args, [n.div_ceil(4), m, 1]);
        }
    }
    stream.mark("end");
    stream.submit_wait().unwrap();
    stream.submit_wait().unwrap(); // Replay must overwrite bias and scratch.
    let output = device.download(&dst, 0, nn * mm * 4 + 64).unwrap();
    assert_eq!(&output[nn * mm * 4..], &[0x5a; 64], "output guard");
    let got: Vec<_> = output[..nn * mm * 4].chunks_exact(4)
        .map(|v| f32::from_le_bytes(v.try_into().unwrap())).collect();
    assert!(got.iter().all(|v| v.is_finite()));
    let indices: Vec<_> = if benchmark { vec![0, nn - 1, (mm - 1) * nn, nn * mm - 1] }
        else { (0..nn * mm).collect() };
    let mut worst = 0.0f64;
    for at in indices {
        let (row, col) = (at / nn, at % nn);
        let mut expected = if bias { b[col] as f64 } else { 0.0 };
        let mut magnitude = expected.abs();
        for d in 0..kk {
            let term = format.weight(&weights, kk, col, d) as f64 * reference_x[row * kk + d] as f64;
            expected += term;
            magnitude += term.abs();
        }
        let error = (got[at] as f64 - expected).abs();
        worst = worst.max(error);
        assert!(error <= 3e-5 + magnitude * 3e-6,
            "{format:?} {n}x{k}x{m} bias={bias} [{row},{col}]: {} vs {expected}, error {error}", got[at]);
    }
    if benchmark {
        for _ in 0..3 { stream.submit_wait().unwrap(); }
        let samples: usize = std::env::var("MAKEPAD_WHISPER_QUANT_SAMPLES").ok()
            .map(|v| v.parse().expect("sample count")).unwrap_or(11);
        assert!(samples > 0);
        let mut times = Vec::new();
        let mut walls = Vec::new();
        for _ in 0..samples {
            let start = Instant::now();
            stream.submit_wait().unwrap();
            walls.push(start.elapsed().as_secs_f64() * 1e3);
            times.push(stream.timings().unwrap()[0].1);
        }
        times.sort_by(f64::total_cmp);
        walls.sort_by(f64::total_cmp);
        let ms = times[times.len() / 2];
        let wall = walls[walls.len() / 2];
        println!("{format:?} [n,k,m]=[{n},{k},{m}] weights={} B median={ms:.4} ms wall={wall:.4} ms {:.2} GOPS max_sample_error={worst:.6}",
            wb.size, 2.0 * n as f64 * k as f64 * m as f64 / (ms * 1e6));
    }
}

#[test]
fn block_matrices_and_vectors_match_cpu() {
    let device = Device::shared().expect("a real Vulkan device is required");
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for format in [BlockQuant::Q8_0, BlockQuant::Q5K, BlockQuant::Q4K] {
        for dims in [[1, 1, 1], [3, 7, 4], [7, 31, 2], [7, 32, 1], [9, 33, 4],
            [65, 65, 1], [5, 1057, 4], [1, 1, 5], [7, 31, 5], [63, 32, 63],
            [65, 33, 67], [69, 96, 71], [7, 1057, 5], [9, 256, 31], [65, 1280, 32], [7, 512, 33]] {
            for bias in [false, true] { run(&device, &mut library, Format::Quant(format), dims, bias, false); }
        }
    }
}

#[test]
#[ignore = "encoder/decoder benchmark; run serially on the target GPU"]
fn whisper_quant_bench() {
    let device = Device::shared().expect("a real Vulkan device is required");
    println!("{}", device.features.description());
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    for format in [Format::F16, Format::Quant(BlockQuant::Q8_0), Format::Quant(BlockQuant::Q5K), Format::Quant(BlockQuant::Q4K)] {
        for dims in [[1280, 1280, 1500], [5120, 1280, 1500], [1280, 5120, 1500]] {
            run(&device, &mut library, format, dims, true, true);
        }
        for [n, k] in [[1280, 1280], [5120, 1280], [1280, 5120], [51866, 1280]] {
            for m in [1, 4] { run(&device, &mut library, format, [n, k, m], true, true); }
        }
    }
}
