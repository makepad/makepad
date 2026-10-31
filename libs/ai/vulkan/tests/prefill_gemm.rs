//! Quantised prefill on a real device. Run the release test binary with
//! `prefill_gemm_bench --ignored --nocapture --test-threads=1`.
//! Optional MAKEPAD_LLM_PREFILL_DIMS="n,k;n,k", _COLUMNS="23,64,128,300,512"
//! and _SAMPLES control the sweep (all use the MAKEPAD_LLM_PREFILL prefix).
use makepad_ai_vulkan::*;
use makepad_script::makepad_math::{f16_bits_to_f32, f32_to_f16_bits};
use std::time::Instant;

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn float(&mut self) -> f32 {
        (self.next() & 65535) as f32 / 32768.0 - 1.0
    }
}

#[derive(Clone, Copy, Debug)]
enum Format {
    Q4K,
    Q5K,
    Q6K,
    Q80,
}
impl Format {
    fn bytes(self) -> usize {
        match self {
            Self::Q4K => 144,
            Self::Q5K => 176,
            Self::Q6K => 224,
            Self::Q80 => 272,
        }
    }
    fn kernel(self) -> &'static str {
        match self {
            Self::Q4K => "gemm_q4k",
            Self::Q5K => "gemm_q5k",
            Self::Q6K => "gemm_q6k",
            Self::Q80 => "gemm_q80",
        }
    }
    fn weights(self, n: usize, k: usize) -> Vec<u8> {
        let mut rng = Rng(0x153_8647);
        let mut weights = vec![0; n * (k / 256) * self.bytes()];
        for (i, block) in weights.chunks_exact_mut(self.bytes()).enumerate() {
            let (size, scale) = match self {
                Self::Q4K => (144, 0),
                Self::Q5K => (176, 0),
                Self::Q6K => (210, 208),
                Self::Q80 => (272, 0),
            };
            for byte in &mut block[..size] {
                *byte = rng.next() as u8;
            }
            let d = if i % 97 == 0 { 0.0 } else { 0.002 + 0.005 * (rng.float() + 1.0) };
            block[scale..scale + 2].copy_from_slice(&f32_to_f16_bits(d).to_le_bytes());
            if matches!(self, Self::Q4K | Self::Q5K) {
                let dmin = 0.001 + 0.0025 * (rng.float() + 1.0);
                block[2..4].copy_from_slice(&f32_to_f16_bits(dmin).to_le_bytes());
            }
            if matches!(self, Self::Q80) {
                // Repacked: eight independent half scales, then 256 codes.
                for j in 0..8 {
                    let d = if (i * 8 + j) % 97 == 0 { 0.0 } else { 0.002 + 0.005 * (rng.float() + 1.0) };
                    block[j * 2..j * 2 + 2].copy_from_slice(&f32_to_f16_bits(d).to_le_bytes());
                }
            }
        }
        weights
    }
    // Scalar GGUF reference, independent of the kernel's packed-word decode.
    fn row(self, bytes: &[u8]) -> Vec<f32> {
        let mut out = Vec::with_capacity(bytes.len() / self.bytes() * 256);
        for block in bytes.chunks_exact(self.bytes()) {
            for i in 0..256 {
                let v = match self {
                    Self::Q4K | Self::Q5K => {
                        let j = i / 32;
                        let (sc, mn) = if j < 4 {
                            (block[4 + j] & 63, block[8 + j] & 63)
                        } else {
                            ((block[8 + j] & 15) | ((block[j] >> 6) << 4),
                             (block[8 + j] >> 4) | ((block[4 + j] >> 6) << 4))
                        };
                        let codes = if matches!(self, Self::Q5K) { 48 } else { 16 };
                        let lo = (block[codes + (i / 64) * 32 + i % 32] >> ((j % 2) * 4)) & 15;
                        let hi = if matches!(self, Self::Q5K) { ((block[16 + i % 32] >> j) & 1) * 16 } else { 0 };
                        half(block, 0) * sc as f32 * (lo + hi) as f32 - half(block, 2) * mn as f32
                    }
                    Self::Q6K => {
                        let h = i / 128;
                        let g = (i % 128) / 32;
                        let l = i % 32;
                        let lo = (block[h * 64 + (g % 2) * 32 + l] >> ((g / 2) * 4)) & 15;
                        let hi = (block[128 + h * 32 + l] >> (g * 2)) & 3;
                        let q = (lo | (hi << 4)) as i32 - 32;
                        let sc = block[192 + h * 8 + 2 * g + l / 16] as i8;
                        half(block, 208) * sc as f32 * q as f32
                    }
                    Self::Q80 => half(block, (i / 32) * 2) * (block[16 + i] as i8) as f32,
                };
                out.push(v);
            }
        }
        out
    }
}

fn half(bytes: &[u8], at: usize) -> f32 {
    f16_bits_to_f32(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}
fn floats(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
}
fn upload(device: &Device, bytes: &[u8]) -> Buffer {
    let buffer = device.buffer(bytes.len() as u64, BufferKind::Device).unwrap();
    device.upload(&buffer, 0, bytes).unwrap();
    buffer
}

struct Reference {
    values: Vec<f32>,
    tolerance: Vec<f32>,
}

fn reference(format: Format, weights: &[u8], k: usize, n: usize, m: usize, q: &[u8], scales: &[f32]) -> Reference {
    // Transpose only the CPU reference activations so the inner loop
    // vectorises across columns. Read the actual GPU quantiser output.
    let mut x = vec![0.0f32; k * m];
    for c in 0..m {
        for d in 0..k {
            x[d * m + c] = (q[c * k + d] as i32 - 128) as f32 * scales[(c * k + d) / 32 * 2];
        }
    }
    let mut values = vec![0.0; n * m];
    let mut tolerance = vec![0.0; n * m];
    let mut sum = vec![0.0f64; m];
    let mut magnitude = vec![0.0f64; m];
    let row_bytes = k / 256 * format.bytes();
    for r in 0..n {
        let w = format.row(&weights[r * row_bytes..(r + 1) * row_bytes]);
        sum.fill(0.0);
        magnitude.fill(0.0);
        for (&a, columns) in w.iter().zip(x.chunks_exact(m)) {
            for ((s, mag), &b) in sum.iter_mut().zip(&mut magnitude).zip(columns) {
                let v = a as f64 * b as f64;
                *s += v;
                *mag += v.abs();
            }
        }
        for c in 0..m {
            values[c * n + r] = sum[c] as f32;
            tolerance[c * n + r] = 1e-4 + magnitude[c] as f32 * 2e-6;
        }
    }
    Reference { values, tolerance }
}

#[test]
#[ignore = "full CPU references and GPU timings; run alone on the target device"]
fn prefill_gemm_bench() {
    let device = Device::shared().expect("real Vulkan device required");
    eprintln!("{}", device.features.description());
    let mut library = Library::new(KERNEL_SOURCES).unwrap();
    let dimensions = std::env::var("MAKEPAD_LLM_PREFILL_DIMS").unwrap_or_else(|_| "9216,2560;2560,9216;8192,2560;4096,2560;2560,4096".into());
    let columns = std::env::var("MAKEPAD_LLM_PREFILL_COLUMNS").unwrap_or_else(|_| "23,64,128,300,512".into());
    let columns: Vec<usize> = columns.split(',').map(|s| s.trim().parse().unwrap()).collect();
    let samples: usize = std::env::var("MAKEPAD_LLM_PREFILL_SAMPLES").map(|s| s.parse().unwrap()).unwrap_or(11);
    assert!(samples > 0 && !columns.is_empty() && columns.iter().all(|&m| m > 0));
    let max_m = *columns.iter().max().unwrap();
    eprintln!("PREFILL timing=median wall submit+wait, one GEMM per submission; excludes quantisation, upload, compilation, CPU reference; GPU timestamp also reported. Every output checked against f64 CPU accumulation of the same int8 activations.");
    for format in [Format::Q4K, Format::Q5K, Format::Q6K, Format::Q80] {
        for entry in dimensions.split(';') {
            let dims: Vec<usize> = entry.split(',').map(|s| s.trim().parse().unwrap()).collect();
            let [n, k]: [usize; 2] = dims.try_into().expect("dimensions must be n,k");
            assert!(n > 0 && k > 0 && k % 256 == 0);
            let weights = format.weights(n, k);
            let wb = upload(&device, &weights);
            let mut rng = Rng(0x793_2421);
            let activations: Vec<u8> = (0..max_m * k).flat_map(|i| {
                let value = if (i / 32) % 131 == 0 { 0.0 } else { rng.float() };
                value.to_le_bytes()
            }).collect();
            let xb = upload(&device, &activations);
            let xq = device.buffer((k * max_m) as u64, BufferKind::Device).unwrap();
            let xs = device.buffer((k / 32 * max_m * 8) as u64, BufferKind::Device).unwrap();
            let quant = library.get(&device, "quantize_q8").unwrap();
            let mut qs = device.stream(0).unwrap();
            qs.begin().unwrap();
            qs.dispatch(&quant.args().buffer("x", xb.address).buffer("xq", xq.address).buffer("xs", xs.address)
                .u32("blocks", (k / 32 * max_m) as u32).u32("k", k as u32).u32("x_col", k as u32),
                [((k / 32 * max_m) as u32).div_ceil(64), 1, 1]);
            qs.submit_wait().unwrap();
            let q = device.download(&xq, 0, k * max_m).unwrap();
            let scales = floats(&device.download(&xs, 0, k / 32 * max_m * 8).unwrap());
            eprintln!("PREFILL reference {format:?} n={n} k={k} m={max_m}");
            let expected = reference(format, &weights, k, n, max_m, &q, &scales);
            for &m in &columns {
                // A nontrivial output stride and guards also catch tail writes.
                let d_col = n + 3;
                let output_bytes = (d_col * m + 16) * 4;
                let dst = device.buffer(output_bytes as u64, BufferKind::Device).unwrap();
                for (variant, tn, tm) in [("main", 32, 32), ("r8k16", 64, 64)] {
                    device.upload(&dst, 0, &vec![0x5a; output_bytes]).unwrap();
                    let name = if variant == "main" { format.kernel().to_string() } else { format!("{}_{variant}", format.kernel()) };
                    let kernel = library.get(&device, &name).unwrap();
                    eprintln!("PREFILL pipeline {name} shared_bytes={} workgroup={:?}", kernel.layout.shared_bytes, kernel.workgroup());
                    let mut stream = device.stream(2).unwrap();
                    stream.begin().unwrap();
                    stream.mark("gemm");
                    stream.dispatch(&kernel.args().buffer("w", wb.address).buffer("w4", wb.address)
                        .buffer("xq", xq.address).buffer("xq4", xq.address).buffer("xs", xs.address).buffer("dst", dst.address)
                        .u32("k", k as u32).u32("n", n as u32).u32("m", m as u32)
                        .u32("w_row", (k / 256 * format.bytes()) as u32).u32("d_col", d_col as u32),
                        [(n as u32).div_ceil(tn), (m as u32).div_ceil(tm), 1]);
                    stream.mark("end");
                    stream.submit_wait().unwrap();
                    let out = device.download(&dst, 0, output_bytes).unwrap();
                    let mut max_error = 0.0f32;
                    let mut worst_budget = 0.0f32;
                    for c in 0..m {
                        for r in 0..n {
                            let at = (c * d_col + r) * 4;
                            let value = f32::from_le_bytes(out[at..at + 4].try_into().unwrap());
                            let error = (value - expected.values[c * n + r]).abs();
                            assert!(value.is_finite(), "{name} n={n} k={k} m={m} ({r},{c}) nonfinite");
                            max_error = max_error.max(error);
                            worst_budget = worst_budget.max(error / expected.tolerance[c * n + r]);
                        }
                        assert!(out[(c * d_col + n) * 4..(c + 1) * d_col * 4].iter().all(|&b| b == 0x5a), "{name}: column guard");
                    }
                    assert!(out[d_col * m * 4..].iter().all(|&b| b == 0x5a), "{name}: output guard");
                    assert!(worst_budget <= 1.0, "{name} n={n} k={k} m={m} max_error={max_error} budget_ratio={worst_budget}");
                    for _ in 0..3 { stream.submit_wait().unwrap(); }
                    let mut wall = Vec::with_capacity(samples);
                    let mut gpu = Vec::with_capacity(samples);
                    for _ in 0..samples {
                        let start = Instant::now();
                        stream.submit_wait().unwrap();
                        wall.push(start.elapsed().as_secs_f64() * 1e3);
                        gpu.push(stream.timings().unwrap()[0].1);
                    }
                    wall.sort_by(f64::total_cmp);
                    gpu.sort_by(f64::total_cmp);
                    let ms = wall[samples / 2];
                    let gops = 2.0 * n as f64 * k as f64 * m as f64 / (ms * 1e6);
                    println!("PREFILL {format:?} n={n} k={k} m={m} {variant} ms={ms:.4} GOPS={gops:.2} gpu_ms={:.4} max_error={max_error:.7} budget_ratio={worst_budget:.5}", gpu[samples / 2]);
                }
            }
        }
    }
}
