// Compression benchmark: fast_inflate vs C libdeflater vs miniz_oxide
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
    level_c: i32,
    level_miniz: u8,
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

    // --- C libdeflater ---
    let start = Instant::now();
    let mut c_compressed_len = 0;
    for _ in 0..iterations {
        let mut comp =
            libdeflater::Compressor::new(libdeflater::CompressionLvl::new(level_c).unwrap());
        let bound = comp.zlib_compress_bound(input_len);
        let mut out = vec![0u8; bound];
        c_compressed_len = comp.zlib_compress(data, &mut out).unwrap();
    }
    let c_elapsed = start.elapsed();
    let c_ns = c_elapsed.as_nanos() as f64 / iterations as f64;
    let c_throughput = input_len as f64 / (c_ns / 1e9) / 1e6;

    // --- miniz_oxide ---
    let start = Instant::now();
    let mut miniz_compressed_len = 0;
    for _ in 0..iterations {
        let out = miniz_oxide::deflate::compress_to_vec_zlib(data, level_miniz);
        miniz_compressed_len = out.len();
    }
    let miniz_elapsed = start.elapsed();
    let miniz_ns = miniz_elapsed.as_nanos() as f64 / iterations as f64;
    let miniz_throughput = input_len as f64 / (miniz_ns / 1e9) / 1e6;

    let ratio_vs_c = rust_ns / c_ns;
    let ratio_vs_miniz = miniz_ns / rust_ns;

    println!(
        "{:>20}  Rust: {:>7.1} MB/s ({:>5}b)  C: {:>7.1} MB/s ({:>5}b)  miniz: {:>7.1} MB/s ({:>5}b)  Rust/C: {:.2}x  miniz/Rust: {:.2}x",
        label,
        rust_throughput, rust_compressed_len,
        c_throughput, c_compressed_len,
        miniz_throughput, miniz_compressed_len,
        ratio_vs_c, ratio_vs_miniz,
    );
}

fn main() {
    println!("Compression benchmark: fast_inflate vs C libdeflater vs miniz_oxide");
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
        bench_compress(label, &data, 1, 1, 1, iterations);
    }

    // Level 6 (default)
    println!("\nLevel 6 (default):");
    println!("{}", "-".repeat(140));
    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let iterations = (2_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 6, 6, 6, iterations);
    }

    // Level 9 (best)
    println!("\nLevel 9 (best):");
    println!("{}", "-".repeat(140));
    for &(label, size) in &sizes {
        let data = make_test_data(size);
        let iterations = (1_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 9, 9, 9, iterations);
    }

    // Low compressibility (random) data
    println!("\nRandom data (low compressibility), level 6:");
    println!("{}", "-".repeat(140));
    use rand::Rng;
    let mut rng = rand::thread_rng();
    for &(label, size) in &[
        ("rand 10 KB", 10_000),
        ("rand 100 KB", 100_000),
        ("rand 1 MB", 1_000_000),
    ] {
        let data: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
        let iterations = (2_000_000 / size).max(5) as u32;
        bench_compress(label, &data, 6, 6, 6, iterations);
    }

    println!();
    println!(
        "Rust/C < 1.0 means Rust is faster. miniz/Rust > 1.0 means Rust is faster than miniz."
    );
    println!("Compressed sizes shown in parentheses (lower = better ratio).");
}
