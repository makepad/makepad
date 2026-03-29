use makepad_lz4::{compress_bound, compress_fast_into, decompress_safe};
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

fn bench_buffer(bytes: &[u8], acceleration: usize) -> Result<BenchResult, String> {
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

    let target_raw_bytes = 256usize * 1024 * 1024;
    let iterations = (target_raw_bytes / bytes.len().max(1)).max(32);

    let compress_started = Instant::now();
    for _ in 0..iterations {
        let written = compress_fast_into(bytes, &mut compressed_buf, acceleration)
            .map_err(|err| format!("compression failed during timing: {err}"))?;
        if written != compressed_len {
            return Err("compressed length changed during timing".to_string());
        }
    }
    let compression_elapsed = compress_started.elapsed();

    let mut decoded_buf = vec![0u8; bytes.len()];
    let decompress_started = Instant::now();
    for _ in 0..iterations {
        let decoded = decompress_safe(&compressed_bytes, &mut decoded_buf)
            .map_err(|err| format!("decompression failed during timing: {err}"))?;
        if decoded != bytes.len() {
            return Err("decoded size changed during timing".to_string());
        }
    }
    let decompression_elapsed = decompress_started.elapsed();

    let raw_mebibytes = (bytes.len() * iterations) as f64 / (1024.0 * 1024.0);
    Ok(BenchResult {
        input_bytes: bytes.len(),
        compressed_bytes: compressed_len,
        compression_mbps: raw_mebibytes / compression_elapsed.as_secs_f64().max(1.0e-9),
        decompression_mbps: raw_mebibytes / decompression_elapsed.as_secs_f64().max(1.0e-9),
    })
}

fn bench_sparse_u16(height_map: &XrDepthAlignHeightMap) -> Result<HeightCodecBenchResult, String> {
    let input_bytes = height_map.heights_meters.len() * std::mem::size_of::<f32>();
    let target_raw_bytes = 256usize * 1024 * 1024;
    let iterations = (target_raw_bytes / input_bytes.max(1)).max(32);

    let mut compressed = height_map.compress_sparse_u16();
    let roundtrip = compressed
        .decompress()
        .map_err(|err| format!("sparse_u16 decode failed: {err}"))?;
    if roundtrip.heights_meters.len() != height_map.heights_meters.len() {
        return Err("sparse_u16 roundtrip size mismatch".to_string());
    }
    let wire_bytes = compressed.serialize_bin().len();

    let encode_started = Instant::now();
    for _ in 0..iterations {
        compressed = height_map.compress_sparse_u16();
        black_box(&compressed);
    }
    let encode_elapsed = encode_started.elapsed();

    let decode_started = Instant::now();
    for _ in 0..iterations {
        let decoded = compressed
            .decompress()
            .map_err(|err| format!("sparse_u16 decode failed during timing: {err}"))?;
        black_box(decoded);
    }
    let decode_elapsed = decode_started.elapsed();

    let raw_mebibytes = (input_bytes * iterations) as f64 / (1024.0 * 1024.0);
    Ok(HeightCodecBenchResult {
        input_bytes,
        wire_bytes,
        encode_mbps: raw_mebibytes / encode_elapsed.as_secs_f64().max(1.0e-9),
        decode_mbps: raw_mebibytes / decode_elapsed.as_secs_f64().max(1.0e-9),
    })
}

fn bench_sparse_lossless(
    height_map: &XrDepthAlignHeightMap,
) -> Result<HeightCodecBenchResult, String> {
    let input_bytes = height_map.heights_meters.len() * std::mem::size_of::<f32>();
    let target_raw_bytes = 256usize * 1024 * 1024;
    let iterations = (target_raw_bytes / input_bytes.max(1)).max(32);

    let mut compressed = height_map.compress_sparse_lossless();
    let roundtrip = compressed
        .decompress()
        .map_err(|err| format!("sparse_lossless decode failed: {err}"))?;
    if roundtrip.heights_meters.len() != height_map.heights_meters.len() {
        return Err("sparse_lossless roundtrip size mismatch".to_string());
    }
    let wire_bytes = compressed.serialize_bin().len();

    let encode_started = Instant::now();
    for _ in 0..iterations {
        compressed = height_map.compress_sparse_lossless();
        black_box(&compressed);
    }
    let encode_elapsed = encode_started.elapsed();

    let decode_started = Instant::now();
    for _ in 0..iterations {
        let decoded = compressed
            .decompress()
            .map_err(|err| format!("sparse_lossless decode failed during timing: {err}"))?;
        black_box(decoded);
    }
    let decode_elapsed = decode_started.elapsed();

    let raw_mebibytes = (input_bytes * iterations) as f64 / (1024.0 * 1024.0);
    Ok(HeightCodecBenchResult {
        input_bytes,
        wire_bytes,
        encode_mbps: raw_mebibytes / encode_elapsed.as_secs_f64().max(1.0e-9),
        decode_mbps: raw_mebibytes / decode_elapsed.as_secs_f64().max(1.0e-9),
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

fn bench_dump(path: &Path, acceleration: usize) -> Result<(), String> {
    let file_bytes =
        fs::read(path).map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let pair = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&file_bytes)
        .ok_or_else(|| format!("failed to decode {}", path.display()))?;
    println!("dump: {}", path.display());
    println!("lz4_acceleration: {}", acceleration);
    print_result("file", bench_buffer(&file_bytes, acceleration)?);

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
    print_result("local heights", bench_buffer(&local_height_bytes, acceleration)?);
    print_result("remote heights", bench_buffer(&remote_height_bytes, acceleration)?);
    print_result("combined heights", bench_buffer(&combined_height_bytes, acceleration)?);
    print_height_codec_result("local sparse_u16", bench_sparse_u16(local_height_map)?);
    print_height_codec_result("remote sparse_u16", bench_sparse_u16(remote_height_map)?);
    print_height_codec_result(
        "local sparse_lossless",
        bench_sparse_lossless(local_height_map)?,
    );
    print_height_codec_result(
        "remote sparse_lossless",
        bench_sparse_lossless(remote_height_map)?,
    );
    println!();
    Ok(())
}

fn main() -> Result<(), String> {
    let (options, paths) = parse_args()?;
    for path in paths {
        bench_dump(&path, options.acceleration)?;
    }
    Ok(())
}
