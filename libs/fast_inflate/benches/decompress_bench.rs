// Speed benchmark: Rust fast_inflate
//
// Run with: cargo bench -p makepad-fast-inflate

use std::time::Instant;

fn make_test_data(size: usize) -> Vec<u8> {
    let phrase = b"The quick brown fox jumps over the lazy dog. Makepad is fast! ";
    let mut data = Vec::with_capacity(size);
    while data.len() < size {
        data.extend_from_slice(phrase);
    }
    data.truncate(size);
    data
}

fn bench_decompress(label: &str, compressed: &[u8], expected_len: usize, iterations: u32) {
    // Warm up
    {
        let mut out = vec![0u8; expected_len];
        makepad_fast_inflate::zlib_decompress(compressed, &mut out).unwrap();
    }

    // Rust fast_inflate
    let start = Instant::now();
    for _ in 0..iterations {
        let mut out = vec![0u8; expected_len];
        let _ = makepad_fast_inflate::zlib_decompress(compressed, &mut out).unwrap();
    }
    let rust_elapsed = start.elapsed();
    let rust_ns = rust_elapsed.as_nanos() as f64 / iterations as f64;
    let rust_throughput = expected_len as f64 / (rust_ns / 1e9) / 1e6;

    println!("{:>20}  Rust: {:>8.1} MB/s ({:>7.0}us)", label, rust_throughput, rust_ns / 1000.0);
}

fn main() {
    println!("Decompression benchmark: Rust fast_inflate");
    println!("{}", "=".repeat(140));

    let sizes = [
        ("1 KB", 1_000),
        ("10 KB", 10_000),
        ("100 KB", 100_000),
        ("1 MB", 1_000_000),
        ("10 MB", 10_000_000),
    ];

    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let compressed = makepad_fast_inflate::zlib_compress(&data, 6);

        let iterations = (10_000_000 / size).max(10) as u32;
        bench_decompress(label, &compressed, data.len(), iterations);
    }

    println!();
    println!("Low-compressibility (random) data:");
    println!("{}", "-".repeat(140));

    // Random data (low compressibility)
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut random_byte = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_f491_4f6c_dd1d) as u8
    };
    for &(label, size) in &[
        ("rand 10 KB", 10_000),
        ("rand 100 KB", 100_000),
        ("rand 1 MB", 1_000_000),
    ] {
        let data: Vec<u8> = (0..size).map(|_| random_byte()).collect();
        let compressed = makepad_fast_inflate::zlib_compress(&data, 6);

        let iterations = (10_000_000 / size).max(10) as u32;
        bench_decompress(label, &compressed, data.len(), iterations);
    }

    println!();
}
