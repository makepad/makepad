// Compression benchmark: fast_inflate
//
// Run with: cargo bench -p makepad-fast-inflate --bench compress_bench

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

fn bench_compress(
    label: &str,
    data: &[u8],
    level_rust: u32,
    iterations: u32,
) {
    let input_len = data.len();

    // --- Rust fast_inflate ---
    // Warm up
    let _ = makepad_fast_inflate::zlib_compress(data, level_rust);

    let start = Instant::now();
    let mut rust_compressed_len = 0;
    for _ in 0..iterations {
        let out = makepad_fast_inflate::zlib_compress(data, level_rust);
        rust_compressed_len = out.len();
    }
    let rust_elapsed = start.elapsed();
    let rust_ns = rust_elapsed.as_nanos() as f64 / iterations as f64;
    let rust_throughput = input_len as f64 / (rust_ns / 1e9) / 1e6;

    println!("{:>20}  Rust: {:>8.1} MB/s ({:>7.0}us)", label, rust_throughput, rust_ns / 1000.0);
    println!("  Compressed size: {rust_compressed_len} bytes");
}

fn main() {
    println!("Compression benchmark: fast_inflate");
    println!("{}", "=".repeat(140));

    let sizes = [
        ("1 KB", 1_000),
        ("10 KB", 10_000),
        ("100 KB", 100_000),
        ("1 MB", 1_000_000),
    ];

    // Level 1 (fastest)
    println!("\nLevel 1 (fastest):");
    println!("{}", "-".repeat(140));
    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let iterations = (5_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 1, iterations);
    }

    // Level 6 (default)
    println!("\nLevel 6 (default):");
    println!("{}", "-".repeat(140));
    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let iterations = (2_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 6, iterations);
    }

    // Level 9 (best)
    println!("\nLevel 9 (best):");
    println!("{}", "-".repeat(140));
    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let iterations = (1_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 9, iterations);
    }

    // Low compressibility (random) data
    println!("\nRandom data (low compressibility), level 6:");
    println!("{}", "-".repeat(140));
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
        let iterations = (2_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 6, iterations);
    }

    println!();
    println!("Compressed sizes shown in parentheses (lower = better ratio).");
}
