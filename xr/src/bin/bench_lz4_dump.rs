use makepad_lz4::{compress_bound, compress_fast_into, decompress_safe, implementation_name};
use makepad_xr::*;
use makepad_xr::makepad_micro_serde::SerBin;
use std::{
    hint::black_box,
    env, fs,
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Copy, Debug)]
struct BenchOptions {
    latest_count: usize,
    acceleration: usize,
    timed_mib: usize,
    samples: usize,
}

#[derive(Clone, Copy, Debug)]
struct BenchResult {
    input_bytes: usize,
    compressed_bytes: usize,
    compression_mbps: f64,
    decompression_mbps: f64,
}

#[derive(Clone, Copy, Debug)]
struct HeightCodecBenchResult {
    input_bytes: usize,
    wire_bytes: usize,
    encode_mbps: f64,
    decode_mbps: f64,
}

fn latest_dump_paths(count: usize) -> Vec<PathBuf> {
    let dump_dir = PathBuf::from("xr/util/dumps");
    let mut entries = fs::read_dir(dump_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then_some((entry.path(), metadata.modified().ok()?))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.1.cmp(&left.1));
    entries
        .into_iter()
        .filter_map(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".bin") && name != "manual-smoke.bin")
                .then_some(path)
        })
        .take(count.max(1))
        .collect()
}

fn parse_args() -> Result<(BenchOptions, Vec<PathBuf>), String> {
    let mut options = BenchOptions {
        latest_count: 2,
        acceleration: 1,
        timed_mib: 512,
        samples: 5,
    };
    let mut paths = Vec::<PathBuf>::new();
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--latest" {
            let Some(count) = args.next() else {
                return Err("expected a count after --latest".to_string());
            };
            let count = count
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "failed to parse --latest count".to_string())?;
            options.latest_count = count.max(1);
            continue;
        }
        if arg == "--accel" {
            let Some(value) = args.next() else {
                return Err("expected a value after --accel".to_string());
            };
            let acceleration = value
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "failed to parse --accel".to_string())?;
            options.acceleration = acceleration.max(1);
            continue;
        }
        if arg == "--timed-mib" {
            let Some(value) = args.next() else {
                return Err("expected a value after --timed-mib".to_string());
            };
            let timed_mib = value
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "failed to parse --timed-mib".to_string())?;
            options.timed_mib = timed_mib.max(1);
            continue;
        }
        if arg == "--samples" {
            let Some(value) = args.next() else {
                return Err("expected a value after --samples".to_string());
            };
            let samples = value
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "failed to parse --samples".to_string())?;
            options.samples = samples.max(1);
            continue;
        }
        paths.push(PathBuf::from(arg));
    }
    if paths.is_empty() {
        paths = latest_dump_paths(options.latest_count);
    }
    if paths.is_empty() {
        return Err("no dump files found in xr/util/dumps".to_string());
    }
    Ok((options, paths))
}

fn height_bytes(height_map: &XrDepthAlignHeightMap) -> Vec<u8> {
    let mut bytes = Vec::<u8>::with_capacity(height_map.heights_meters.len() * 4);
    for height in &height_map.heights_meters {
        bytes.extend_from_slice(&height.to_le_bytes());
    }
    bytes
}

fn target_raw_bytes(options: BenchOptions) -> usize {
    options.timed_mib * 1024 * 1024
}

fn timing_iterations(input_bytes: usize, options: BenchOptions) -> usize {
    target_raw_bytes(options).div_ceil(input_bytes.max(1)).max(1)
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    let mid = samples.len() / 2;
    if samples.len() % 2 == 0 {
        (samples[mid - 1] + samples[mid]) * 0.5
    } else {
        samples[mid]
    }
}

fn bench_buffer(bytes: &[u8], acceleration: usize, options: BenchOptions) -> Result<BenchResult, String> {
    let bound = compress_bound(bytes.len());
    if bound == 0 {
        return Err(format!("buffer too large for lz4: {} bytes", bytes.len()));
    }
    let mut compressed_buf = vec![0u8; bound];
    let compressed_len = compress_fast_into(bytes, &mut compressed_buf, acceleration)
        .map_err(|err| format!("compression failed: {err}"))?;
    let compressed_bytes = compressed_buf[..compressed_len].to_vec();
    let mut roundtrip = vec![0u8; bytes.len()];
    let decoded = decompress_safe(&compressed_bytes, &mut roundtrip)
        .map_err(|err| format!("decompression failed: {err}"))?;
    if decoded != bytes.len() || roundtrip != bytes {
        return Err("lz4 roundtrip mismatch".to_string());
    }

    let iterations = timing_iterations(bytes.len(), options);
    let raw_mebibytes = (bytes.len() * iterations) as f64 / (1024.0 * 1024.0);
    let mut compression_samples = Vec::<f64>::with_capacity(options.samples);
    let mut decompression_samples = Vec::<f64>::with_capacity(options.samples);

    for _ in 0..options.samples {
        let compress_started = Instant::now();
        for _ in 0..iterations {
            let written = compress_fast_into(bytes, &mut compressed_buf, acceleration)
                .map_err(|err| format!("compression failed during timing: {err}"))?;
            if written != compressed_len {
                return Err("compressed length changed during timing".to_string());
            }
        }
        compression_samples.push(raw_mebibytes / compress_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    let mut decoded_buf = vec![0u8; bytes.len()];
    for _ in 0..options.samples {
        let decompress_started = Instant::now();
        for _ in 0..iterations {
            let decoded = decompress_safe(&compressed_bytes, &mut decoded_buf)
                .map_err(|err| format!("decompression failed during timing: {err}"))?;
            if decoded != bytes.len() {
                return Err("decoded size changed during timing".to_string());
            }
        }
        decompression_samples
            .push(raw_mebibytes / decompress_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    Ok(BenchResult {
        input_bytes: bytes.len(),
        compressed_bytes: compressed_len,
        compression_mbps: median(&mut compression_samples),
        decompression_mbps: median(&mut decompression_samples),
    })
}

fn bench_sparse_u16(
    height_map: &XrDepthAlignHeightMap,
    options: BenchOptions,
) -> Result<HeightCodecBenchResult, String> {
    let input_bytes = height_map.heights_meters.len() * std::mem::size_of::<f32>();
    let iterations = timing_iterations(input_bytes, options);

    let mut compressed = height_map.compress_sparse_u16();
    let roundtrip = compressed
        .decompress()
        .map_err(|err| format!("sparse_u16 decode failed: {err}"))?;
    if roundtrip.heights_meters.len() != height_map.heights_meters.len() {
        return Err("sparse_u16 roundtrip size mismatch".to_string());
    }
    let wire_bytes = compressed.serialize_bin().len();
    let raw_mebibytes = (input_bytes * iterations) as f64 / (1024.0 * 1024.0);
    let mut encode_samples = Vec::<f64>::with_capacity(options.samples);
    let mut decode_samples = Vec::<f64>::with_capacity(options.samples);

    for _ in 0..options.samples {
        let encode_started = Instant::now();
        for _ in 0..iterations {
            compressed = height_map.compress_sparse_u16();
            black_box(&compressed);
        }
        encode_samples.push(raw_mebibytes / encode_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    for _ in 0..options.samples {
        let decode_started = Instant::now();
        for _ in 0..iterations {
            let decoded = compressed
                .decompress()
                .map_err(|err| format!("sparse_u16 decode failed during timing: {err}"))?;
            black_box(decoded);
        }
        decode_samples.push(raw_mebibytes / decode_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    Ok(HeightCodecBenchResult {
        input_bytes,
        wire_bytes,
        encode_mbps: median(&mut encode_samples),
        decode_mbps: median(&mut decode_samples),
    })
}

fn bench_sparse_lossless(
    height_map: &XrDepthAlignHeightMap,
    options: BenchOptions,
) -> Result<HeightCodecBenchResult, String> {
    let input_bytes = height_map.heights_meters.len() * std::mem::size_of::<f32>();
    let iterations = timing_iterations(input_bytes, options);

    let mut compressed = height_map.compress_sparse_lossless();
    let roundtrip = compressed
        .decompress()
        .map_err(|err| format!("sparse_lossless decode failed: {err}"))?;
    if roundtrip.heights_meters.len() != height_map.heights_meters.len() {
        return Err("sparse_lossless roundtrip size mismatch".to_string());
    }
    let wire_bytes = compressed.serialize_bin().len();
    let raw_mebibytes = (input_bytes * iterations) as f64 / (1024.0 * 1024.0);
    let mut encode_samples = Vec::<f64>::with_capacity(options.samples);
    let mut decode_samples = Vec::<f64>::with_capacity(options.samples);

    for _ in 0..options.samples {
        let encode_started = Instant::now();
        for _ in 0..iterations {
            compressed = height_map.compress_sparse_lossless();
            black_box(&compressed);
        }
        encode_samples.push(raw_mebibytes / encode_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    for _ in 0..options.samples {
        let decode_started = Instant::now();
        for _ in 0..iterations {
            let decoded = compressed
                .decompress()
                .map_err(|err| format!("sparse_lossless decode failed during timing: {err}"))?;
            black_box(decoded);
        }
        decode_samples.push(raw_mebibytes / decode_started.elapsed().as_secs_f64().max(1.0e-9));
    }

    Ok(HeightCodecBenchResult {
        input_bytes,
        wire_bytes,
        encode_mbps: median(&mut encode_samples),
        decode_mbps: median(&mut decode_samples),
    })
}

fn print_result(label: &str, result: BenchResult) {
    let ratio = result.compressed_bytes as f64 / result.input_bytes.max(1) as f64;
    let saved = 100.0 * (1.0 - ratio);
    println!(
        "{label}: {} -> {} bytes | ratio {:.3}x | saved {:.1}% | compress {:.0} MiB/s | decompress {:.0} MiB/s",
        result.input_bytes,
        result.compressed_bytes,
        ratio,
        saved,
        result.compression_mbps,
        result.decompression_mbps,
    );
}

fn print_height_codec_result(label: &str, result: HeightCodecBenchResult) {
    let ratio = result.wire_bytes as f64 / result.input_bytes.max(1) as f64;
    let saved = 100.0 * (1.0 - ratio);
    println!(
        "{label}: {} -> {} bytes | ratio {:.3}x | saved {:.1}% | encode {:.0} MiB/s | decode {:.0} MiB/s",
        result.input_bytes,
        result.wire_bytes,
        ratio,
        saved,
        result.encode_mbps,
        result.decode_mbps,
    );
}

fn bench_dump(path: &Path, options: BenchOptions) -> Result<(), String> {
    let file_bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    println!("dump: {}", path.display());
    println!("lz4_impl: {}", implementation_name());
    println!(
        "lz4_acceleration: {} | timed_mib: {} | samples: {}",
        options.acceleration, options.timed_mib, options.samples
    );
    print_result("file", bench_buffer(&file_bytes, options.acceleration, options)?);

    let Some(pair) = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&file_bytes) else {
        println!();
        return Ok(());
    };

    let local_height_map = pair
        .local_descriptor
        .descriptor
        .height_map
        .as_ref()
        .ok_or_else(|| "local descriptor has no height map".to_string())?;
    let remote_height_map = pair
        .remote_descriptor
        .descriptor
        .height_map
        .as_ref()
        .ok_or_else(|| "remote descriptor has no height map".to_string())?;
    let local_height_bytes = height_bytes(local_height_map);
    let remote_height_bytes = height_bytes(remote_height_map);
    let mut combined_height_bytes =
        Vec::<u8>::with_capacity(local_height_bytes.len() + remote_height_bytes.len());
    combined_height_bytes.extend_from_slice(&local_height_bytes);
    combined_height_bytes.extend_from_slice(&remote_height_bytes);
    print_result(
        "local heights",
        bench_buffer(&local_height_bytes, options.acceleration, options)?,
    );
    print_result(
        "remote heights",
        bench_buffer(&remote_height_bytes, options.acceleration, options)?,
    );
    print_result(
        "combined heights",
        bench_buffer(&combined_height_bytes, options.acceleration, options)?,
    );
    print_height_codec_result("local sparse_u16", bench_sparse_u16(local_height_map, options)?);
    print_height_codec_result(
        "remote sparse_u16",
        bench_sparse_u16(remote_height_map, options)?,
    );
    print_height_codec_result(
        "local sparse_lossless",
        bench_sparse_lossless(local_height_map, options)?,
    );
    print_height_codec_result(
        "remote sparse_lossless",
        bench_sparse_lossless(remote_height_map, options)?,
    );
    println!();
    Ok(())
}

fn main() -> Result<(), String> {
    let (options, paths) = parse_args()?;
    for path in paths {
        bench_dump(&path, options)?;
    }
    Ok(())
}
