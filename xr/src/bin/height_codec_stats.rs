use makepad_xr::*;
use makepad_xr::makepad_micro_serde::SerBin;
use std::{fs, path::PathBuf, time::Instant};

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

#[derive(Default)]
struct RunStats {
    runs: usize,
    mean: f64,
    p50: usize,
    p90: usize,
    p99: usize,
    max: usize,
}

#[derive(Default)]
struct TileStats {
    total: usize,
    empty: usize,
    full: usize,
    mixed: usize,
}

#[derive(Default)]
struct TileRangeStats {
    non_empty_tiles: usize,
    mask_bytes_8x8: usize,
    tile_header_bytes_8x8: usize,
    local_u8_sideband_bytes_8x8: usize,
    p50_range: f32,
    p90_range: f32,
    p99_range: f32,
    max_range: f32,
    p50_u8_error: f32,
    p90_u8_error: f32,
    p99_u8_error: f32,
    max_u8_error: f32,
}

fn quantize_error(height_map: &XrDepthAlignHeightMap, levels: u32) -> (f32, f32) {
    let range = (height_map.top_y_meters - height_map.bottom_y_meters).max(1.0e-6);
    let denom = (levels.saturating_sub(1)).max(1) as f32;
    let mut max_abs_error = 0.0f32;
    let mut sum_sq_error = 0.0f64;
    let mut count = 0usize;
    for value in height_map.heights_meters.iter().copied().filter(|value| value.is_finite()) {
        let normalized = ((value - height_map.bottom_y_meters) / range).clamp(0.0, 1.0);
        let q = (normalized * denom).round();
        let decoded = height_map.bottom_y_meters + (q / denom) * range;
        let error = (decoded - value).abs();
        max_abs_error = max_abs_error.max(error);
        sum_sq_error += f64::from(error * error);
        count += 1;
    }
    let rmse = if count == 0 {
        0.0
    } else {
        (sum_sq_error / count as f64).sqrt() as f32
    };
    (max_abs_error, rmse)
}

fn collect_runs(bits: impl Iterator<Item = bool>) -> (RunStats, RunStats) {
    let mut zeros = Vec::<usize>::new();
    let mut ones = Vec::<usize>::new();
    let mut current = None::<(bool, usize)>;
    for bit in bits {
        match current {
            Some((value, len)) if value == bit => {
                current = Some((value, len + 1));
            }
            Some((value, len)) => {
                if value {
                    ones.push(len);
                } else {
                    zeros.push(len);
                }
                current = Some((bit, 1));
            }
            None => current = Some((bit, 1)),
        }
    }
    if let Some((value, len)) = current {
        if value {
            ones.push(len);
        } else {
            zeros.push(len);
        }
    }
    (summarize_runs(&zeros), summarize_runs(&ones))
}

fn summarize_runs(values: &[usize]) -> RunStats {
    if values.is_empty() {
        return RunStats::default();
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let sum = sorted.iter().copied().sum::<usize>();
    let percentile = |t: f32| -> usize {
        let last = sorted.len().saturating_sub(1);
        let index = ((last as f32) * t).round() as usize;
        sorted[index]
    };
    RunStats {
        runs: sorted.len(),
        mean: sum as f64 / sorted.len() as f64,
        p50: percentile(0.50),
        p90: percentile(0.90),
        p99: percentile(0.99),
        max: *sorted.last().unwrap_or(&0),
    }
}

fn tile_stats(height_map: &XrDepthAlignHeightMap, tile_edge: usize) -> TileStats {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    let tiles_x = size_x.div_ceil(tile_edge);
    let tiles_z = size_z.div_ceil(tile_edge);
    let mut stats = TileStats::default();
    for tile_z in 0..tiles_z {
        for tile_x in 0..tiles_x {
            stats.total += 1;
            let mut valid_count = 0usize;
            let z_start = tile_z * tile_edge;
            let z_end = (z_start + tile_edge).min(size_z);
            let x_start = tile_x * tile_edge;
            let x_end = (x_start + tile_edge).min(size_x);
            for z in z_start..z_end {
                for x in x_start..x_end {
                    let index = height_map.cell_index(x, z);
                    if height_map.heights_meters[index].is_finite() {
                        valid_count += 1;
                    }
                }
            }
            let tile_cells = (z_end - z_start) * (x_end - x_start);
            if valid_count == 0 {
                stats.empty += 1;
            } else if valid_count == tile_cells {
                stats.full += 1;
            } else {
                stats.mixed += 1;
            }
        }
    }
    stats
}

fn tile_range_stats_8x8(height_map: &XrDepthAlignHeightMap) -> TileRangeStats {
    let tile_edge = 8usize;
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    let tiles_x = size_x.div_ceil(tile_edge);
    let tiles_z = size_z.div_ceil(tile_edge);
    let mut ranges = Vec::<f32>::new();
    let mut errors = Vec::<f32>::new();
    let mut mixed_tiles = 0usize;
    let mut non_empty_tiles = 0usize;

    for tile_z in 0..tiles_z {
        for tile_x in 0..tiles_x {
            let z_start = tile_z * tile_edge;
            let z_end = (z_start + tile_edge).min(size_z);
            let x_start = tile_x * tile_edge;
            let x_end = (x_start + tile_edge).min(size_x);

            let mut min_value = f32::INFINITY;
            let mut max_value = f32::NEG_INFINITY;
            let mut valid_count = 0usize;
            let tile_cell_count = (z_end - z_start) * (x_end - x_start);
            for z in z_start..z_end {
                for x in x_start..x_end {
                    let value = height_map.heights_meters[height_map.cell_index(x, z)];
                    if value.is_finite() {
                        valid_count += 1;
                        min_value = min_value.min(value);
                        max_value = max_value.max(value);
                    }
                }
            }
            if valid_count == 0 {
                continue;
            }
            non_empty_tiles += 1;
            if valid_count != tile_cell_count {
                mixed_tiles += 1;
            }
            let range = (max_value - min_value).max(0.0);
            ranges.push(range);
            let denom = 255.0f32;
            let step = if range <= 1.0e-6 { 0.0 } else { range / denom };
            errors.push(step * 0.5);
        }
    }

    if ranges.is_empty() {
        return TileRangeStats::default();
    }
    ranges.sort_by(|left, right| left.total_cmp(right));
    errors.sort_by(|left, right| left.total_cmp(right));
    let percentile_f32 = |values: &[f32], t: f32| -> f32 {
        let last = values.len().saturating_sub(1);
        let index = ((last as f32) * t).round() as usize;
        values[index]
    };
    TileRangeStats {
        non_empty_tiles,
        mask_bytes_8x8: (tiles_x * tiles_z * 2).div_ceil(8) + mixed_tiles * 8,
        tile_header_bytes_8x8: (tiles_x * tiles_z * 2).div_ceil(8),
        local_u8_sideband_bytes_8x8: non_empty_tiles * 4,
        p50_range: percentile_f32(&ranges, 0.50),
        p90_range: percentile_f32(&ranges, 0.90),
        p99_range: percentile_f32(&ranges, 0.99),
        max_range: *ranges.last().unwrap_or(&0.0),
        p50_u8_error: percentile_f32(&errors, 0.50),
        p90_u8_error: percentile_f32(&errors, 0.90),
        p99_u8_error: percentile_f32(&errors, 0.99),
        max_u8_error: *errors.last().unwrap_or(&0.0),
    }
}

fn cheap_rle_mask_bytes(height_map: &XrDepthAlignHeightMap) -> usize {
    let mut total_bytes = 0usize;
    let mut current = None::<(bool, usize)>;
    for bit in height_map
        .heights_meters
        .iter()
        .map(|value| value.is_finite())
    {
        match current {
            Some((value, len)) if value == bit && len < u16::MAX as usize => {
                current = Some((value, len + 1));
            }
            Some((_value, len)) => {
                let _ = len;
                total_bytes += 3;
                current = Some((bit, 1));
            }
            None => current = Some((bit, 1)),
        }
    }
    if current.is_some() {
        total_bytes += 3;
    }
    total_bytes
}

fn neighbor_delta_stats(height_map: &XrDepthAlignHeightMap) -> (f32, f32, f32) {
    let size_x = height_map.size_x_usize();
    let size_z = height_map.size_z_usize();
    let mut deltas = Vec::<f32>::new();
    for z in 0..size_z {
        for x in 0..size_x {
            let center = height_map.heights_meters[height_map.cell_index(x, z)];
            if !center.is_finite() {
                continue;
            }
            if x + 1 < size_x {
                let right = height_map.heights_meters[height_map.cell_index(x + 1, z)];
                if right.is_finite() {
                    deltas.push((right - center).abs());
                }
            }
            if z + 1 < size_z {
                let down = height_map.heights_meters[height_map.cell_index(x, z + 1)];
                if down.is_finite() {
                    deltas.push((down - center).abs());
                }
            }
        }
    }
    if deltas.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    deltas.sort_by(|left, right| left.total_cmp(right));
    let percentile = |t: f32| -> f32 {
        let last = deltas.len().saturating_sub(1);
        let index = ((last as f32) * t).round() as usize;
        deltas[index]
    };
    (percentile(0.50), percentile(0.90), percentile(0.99))
}

fn print_map_stats(label: &str, height_map: &XrDepthAlignHeightMap) {
    let cell_count = height_map.cell_count();
    let valid_count = height_map
        .heights_meters
        .iter()
        .filter(|value| value.is_finite())
        .count();
    let bitmask_bytes = cell_count.div_ceil(8);
    let rle_mask_bytes = cheap_rle_mask_bytes(height_map);
    let (invalid_runs, valid_runs) = collect_runs(
        height_map
            .heights_meters
            .iter()
            .copied()
            .map(|value| value.is_finite()),
    );
    let tiles_8 = tile_stats(height_map, 8);
    let tiles_16 = tile_stats(height_map, 16);
    let tile_ranges_8 = tile_range_stats_8x8(height_map);
    let (delta_p50, delta_p90, delta_p99) = neighbor_delta_stats(height_map);
    let (u8_max, u8_rmse) = quantize_error(height_map, 256);
    let (u12_max, u12_rmse) = quantize_error(height_map, 4096);
    let (u16_max, u16_rmse) = quantize_error(height_map, 65536);
    let codec_passes = 128usize;
    let encode_started = Instant::now();
    let mut compressed = height_map.compress_sparse_u16();
    for _ in 1..codec_passes {
        compressed = height_map.compress_sparse_u16();
    }
    let encode_micros = encode_started.elapsed().as_micros() as f64 / codec_passes as f64;
    let compressed_bytes = compressed.serialize_bin().len();
    let decode_started = Instant::now();
    let mut decoded = XrDepthAlignHeightMap::default();
    for _ in 0..codec_passes {
        decoded = compressed
            .decompress()
            .expect("compressed height map should decode");
    }
    let decode_micros = decode_started.elapsed().as_micros() as f64 / codec_passes as f64;
    let mut codec_max_error = 0.0f32;
    for (expected, actual) in height_map
        .heights_meters
        .iter()
        .copied()
        .zip(decoded.heights_meters.iter().copied())
    {
        if expected.is_finite() {
            codec_max_error = codec_max_error.max((expected - actual).abs());
        }
    }
    let mut floor_band_1cm = 0usize;
    let mut floor_band_2cm = 0usize;
    let mut floor_band_5cm = 0usize;
    let mut floor_band_10cm = 0usize;
    let mut floor_band_20cm = 0usize;
    let mut above_1m = 0usize;
    let mut above_150cm = 0usize;
    for value in height_map.heights_meters.iter().copied().filter(|value| value.is_finite()) {
        let delta = value - height_map.floor_y_meters;
        if delta <= 0.01 {
            floor_band_1cm += 1;
        }
        if delta <= 0.02 {
            floor_band_2cm += 1;
        }
        if delta <= 0.05 {
            floor_band_5cm += 1;
        }
        if delta <= 0.10 {
            floor_band_10cm += 1;
        }
        if delta <= 0.20 {
            floor_band_20cm += 1;
        }
        if delta >= 1.0 {
            above_1m += 1;
        }
        if delta >= 1.5 {
            above_150cm += 1;
        }
    }

    println!(
        "{label}: cells {} valid {} ({:.1}%) raw_f32_bytes {}",
        cell_count,
        valid_count,
        100.0 * valid_count as f32 / cell_count.max(1) as f32,
        cell_count * std::mem::size_of::<f32>(),
    );
    println!(
        "{label}: mask bitset {} B | mask cheap_rle {} B | u16+bitset {} B | u8+bitset {} B",
        bitmask_bytes,
        rle_mask_bytes,
        bitmask_bytes + valid_count * 2,
        bitmask_bytes + valid_count,
    );
    println!(
        "{label}: codec_sparse_u16 serialized {} B | encode {:.1} us | decode {:.1} us | max_err {:.6} m",
        compressed_bytes, encode_micros, decode_micros, codec_max_error
    );
    println!(
        "{label}: invalid runs count {} mean {:.1} p50 {} p90 {} p99 {} max {}",
        invalid_runs.runs,
        invalid_runs.mean,
        invalid_runs.p50,
        invalid_runs.p90,
        invalid_runs.p99,
        invalid_runs.max,
    );
    println!(
        "{label}: valid runs count {} mean {:.1} p50 {} p90 {} p99 {} max {}",
        valid_runs.runs,
        valid_runs.mean,
        valid_runs.p50,
        valid_runs.p90,
        valid_runs.p99,
        valid_runs.max,
    );
    println!(
        "{label}: tile8 empty {} full {} mixed {} | tile16 empty {} full {} mixed {}",
        tiles_8.empty,
        tiles_8.full,
        tiles_8.mixed,
        tiles_16.empty,
        tiles_16.full,
        tiles_16.mixed,
    );
    println!(
        "{label}: tile8 non-empty {} mask {} B (headers {} B) | tile8 local-u8 total {} B | tile8 range p50 {:.4} p90 {:.4} p99 {:.4} max {:.4}",
        tile_ranges_8.non_empty_tiles,
        tile_ranges_8.mask_bytes_8x8,
        tile_ranges_8.tile_header_bytes_8x8,
        tile_ranges_8.mask_bytes_8x8
            + tile_ranges_8.local_u8_sideband_bytes_8x8
            + valid_count,
        tile_ranges_8.p50_range,
        tile_ranges_8.p90_range,
        tile_ranges_8.p99_range,
        tile_ranges_8.max_range,
    );
    println!(
        "{label}: tile8 local-u8 max abs err p50 {:.4} p90 {:.4} p99 {:.4} max {:.4}",
        tile_ranges_8.p50_u8_error,
        tile_ranges_8.p90_u8_error,
        tile_ranges_8.p99_u8_error,
        tile_ranges_8.max_u8_error,
    );
    println!(
        "{label}: neighbor abs delta p50 {:.4} m p90 {:.4} m p99 {:.4} m",
        delta_p50, delta_p90, delta_p99
    );
    println!(
        "{label}: floor bands <=1cm {} ({:.1}%) <=2cm {} ({:.1}%) <=5cm {} ({:.1}%) <=10cm {} ({:.1}%) <=20cm {} ({:.1}%)",
        floor_band_1cm,
        100.0 * floor_band_1cm as f32 / valid_count.max(1) as f32,
        floor_band_2cm,
        100.0 * floor_band_2cm as f32 / valid_count.max(1) as f32,
        floor_band_5cm,
        100.0 * floor_band_5cm as f32 / valid_count.max(1) as f32,
        floor_band_10cm,
        100.0 * floor_band_10cm as f32 / valid_count.max(1) as f32,
        floor_band_20cm,
        100.0 * floor_band_20cm as f32 / valid_count.max(1) as f32,
    );
    println!(
        "{label}: above floor >=1.0m {} ({:.1}%) | >=1.5m {} ({:.1}%)",
        above_1m,
        100.0 * above_1m as f32 / valid_count.max(1) as f32,
        above_150cm,
        100.0 * above_150cm as f32 / valid_count.max(1) as f32,
    );
    println!(
        "{label}: abs quant err u8 max {:.4} rmse {:.4} | u12 max {:.4} rmse {:.4} | u16 max {:.6} rmse {:.6}",
        u8_max, u8_rmse, u12_max, u12_rmse, u16_max, u16_rmse
    );
}

fn main() {
    for path in latest_dump_paths(8) {
        let bytes = fs::read(&path).expect("dump read should succeed");
        let pair = XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
            .expect("dump decode should succeed");
        println!("dump: {}", path.display());
        if let Some(local) = pair.local_descriptor.descriptor.height_map.as_ref() {
            print_map_stats("local", local);
        }
        if let Some(remote) = pair.remote_descriptor.descriptor.height_map.as_ref() {
            print_map_stats("remote", remote);
        }
        println!();
    }
}
