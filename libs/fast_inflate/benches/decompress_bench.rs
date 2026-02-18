// Speed benchmark: Rust fast_inflate vs C libdeflater vs miniz_oxide (flate2)
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

    // C libdeflater
    let start = Instant::now();
    for _ in 0..iterations {
        let mut decomp = libdeflater::Decompressor::new();
        let mut out = vec![0u8; expected_len];
        let _ = decomp.zlib_decompress(compressed, &mut out).unwrap();
    }
    let c_elapsed = start.elapsed();
    let c_ns = c_elapsed.as_nanos() as f64 / iterations as f64;
    let c_throughput = expected_len as f64 / (c_ns / 1e9) / 1e6;

    // flate2 (miniz_oxide)
    let start = Instant::now();
    for _ in 0..iterations {
        use std::io::Read;
        let mut decoder = flate2::read::ZlibDecoder::new(compressed);
        let mut out = Vec::with_capacity(expected_len);
        decoder.read_to_end(&mut out).unwrap();
    }
    let miniz_elapsed = start.elapsed();
    let miniz_ns = miniz_elapsed.as_nanos() as f64 / iterations as f64;
    let miniz_throughput = expected_len as f64 / (miniz_ns / 1e9) / 1e6;

    let ratio_vs_c = rust_ns / c_ns;
    let ratio_vs_miniz = miniz_ns / rust_ns;

    println!(
        "{:>20}  Rust: {:>8.1} MB/s ({:>7.0}us)  C: {:>8.1} MB/s ({:>7.0}us)  miniz: {:>8.1} MB/s ({:>7.0}us)  Rust/C: {:.2}x  miniz/Rust: {:.2}x",
        label,
        rust_throughput, rust_ns / 1000.0,
        c_throughput, c_ns / 1000.0,
        miniz_throughput, miniz_ns / 1000.0,
        ratio_vs_c, ratio_vs_miniz,
    );
}

fn main() {
    println!("Decompression benchmark: Rust fast_inflate vs C libdeflater vs miniz_oxide");
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
        let mut comp = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(6).unwrap());
        let max_sz = comp.zlib_compress_bound(data.len());
        let mut compressed = vec![0u8; max_sz];
        let clen = comp.zlib_compress(&data, &mut compressed).unwrap();
        compressed.truncate(clen);

        let iterations = (10_000_000 / size).max(10) as u32;
        bench_decompress(label, &compressed, data.len(), iterations);
    }

    println!();
    println!("Low-compressibility (random) data:");
    println!("{}", "-".repeat(140));

    // Random data (low compressibility)
    use rand::Rng;
    let mut rng = rand::thread_rng();
    for &(label, size) in &[
        ("rand 10 KB", 10_000),
        ("rand 100 KB", 100_000),
        ("rand 1 MB", 1_000_000),
    ] {
        let data: Vec<u8> = (0..size).map(|_| rng.gen()).collect();
        let mut comp = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(6).unwrap());
        let max_sz = comp.zlib_compress_bound(data.len());
        let mut compressed = vec![0u8; max_sz];
        let clen = comp.zlib_compress(&data, &mut compressed).unwrap();
        compressed.truncate(clen);

        let iterations = (10_000_000 / size).max(10) as u32;
        bench_decompress(label, &compressed, data.len(), iterations);
    }

    println!();
    println!(
        "Rust/C < 1.0 means Rust is faster. miniz/Rust > 1.0 means Rust is faster than miniz."
    );
}
