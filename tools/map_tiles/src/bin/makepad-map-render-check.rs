//! Proves a repack policy is invisible to the renderer: for each selected
//! tile, rewrite it with the repack policy and require the renderer to build
//! identical GPU buffers from the original and the rewrite, at every render
//! bucket in 2D and 3D (see makepad_map_build::render_check).

use makepad_map_build::render_check::{check_tile, RenderCheckStats};
use makepad_map_build::repack::rewrite_tile;
use makepad_mbtile_reader::{MkmapReader, TileArchiveReader};
use std::env;
use std::path::PathBuf;
use std::time::Instant;

const USAGE: &str = "Usage: makepad-map-render-check <archive.mkmap | archive.mbtiles> [--tiles z/x/y,...] [--zoom Z] [--largest N] [--every K] [--against converted.mkmap]\n--against compares with the tiles another archive actually stores instead of rewriting on the fly.\nDefaults: --zoom 14 --largest 8 (the N biggest tiles at Z) plus every K-th tile (default 0 = none).";

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Err(USAGE.to_string());
    }
    let path = PathBuf::from(&args[0]);
    let mut explicit = Vec::new();
    let mut zoom = 14_u32;
    let mut largest = 8_usize;
    let mut every = 0_usize;
    let mut against: Option<PathBuf> = None;
    let mut index = 1;
    while index < args.len() {
        let value = args.get(index + 1);
        match args[index].as_str() {
            "--tiles" => {
                for item in value.ok_or("--tiles requires a value")?.split(',') {
                    let parts: Vec<u32> = item
                        .split('/')
                        .map(|part| part.parse::<u32>())
                        .collect::<Result<_, _>>()
                        .map_err(|err| format!("invalid tile '{item}': {err}"))?;
                    if parts.len() != 3 {
                        return Err(format!("invalid tile '{item}', expected z/x/y"));
                    }
                    explicit.push((parts[0], parts[1], parts[2]));
                }
            }
            "--zoom" => zoom = parse(value, "--zoom")?,
            "--largest" => largest = parse(value, "--largest")?,
            "--every" => every = parse(value, "--every")?,
            "--against" => against = Some(PathBuf::from(value.ok_or("--against requires a value")?)),
            other => return Err(format!("unknown argument '{other}'\n{USAGE}")),
        }
        index += 2;
    }

    // (z, x, y, compressed length) candidates.
    let mut candidates: Vec<(u32, u32, u32, u64)> = Vec::new();
    if explicit.is_empty() {
        if TileArchiveReader::is_mkmap_path(&path) {
            let mut reader = MkmapReader::open(&path).map_err(|err| err.to_string())?;
            reader
                .for_each_tile_ref(|tile| {
                    if tile.zoom as u32 == zoom {
                        candidates.push((zoom, tile.x, tile.y, tile.len));
                    }
                })
                .map_err(|err| err.to_string())?;
        } else {
            let mut reader = TileArchiveReader::open(&path).map_err(|err| err.to_string())?;
            for tile in reader
                .get_tiles_at_zoom(zoom as i64)
                .map_err(|err| err.to_string())?
            {
                let y = ((1_i64 << zoom) - 1 - tile.tile_row) as u32;
                candidates.push((zoom, tile.tile_column as u32, y, tile.tile_data.len() as u64));
            }
        }
    }
    let mut selected: Vec<(u32, u32, u32)> = explicit;
    if !candidates.is_empty() {
        candidates.sort_by_key(|tile| std::cmp::Reverse(tile.3));
        selected.extend(candidates.iter().take(largest).map(|t| (t.0, t.1, t.2)));
        if every > 0 {
            selected.extend(
                candidates
                    .iter()
                    .skip(largest)
                    .step_by(every)
                    .map(|t| (t.0, t.1, t.2)),
            );
        }
    }
    if selected.is_empty() {
        return Err("no tiles selected".to_string());
    }

    let mut reader = TileArchiveReader::open(&path).map_err(|err| err.to_string())?;
    let mut converted = against
        .as_ref()
        .map(|path| TileArchiveReader::open(path).map_err(|err| err.to_string()))
        .transpose()?;
    let mut stats = RenderCheckStats::default();
    let (mut bytes_before, mut bytes_after) = (0_u64, 0_u64);
    let start = Instant::now();
    for (z, x, y) in selected {
        let row = (1_i64 << z) - 1 - y as i64;
        let Some(before) = reader
            .get_tile_decoded(z as i64, x as i64, row)
            .map_err(|err| format!("read {z}/{x}/{y}: {err}"))?
        else {
            println!("{z}/{x}/{y}: not in archive");
            continue;
        };
        let after = match converted.as_mut() {
            Some(converted) => converted
                .get_tile_decoded(z as i64, x as i64, row)
                .map_err(|err| format!("read converted {z}/{x}/{y}: {err}"))?
                .ok_or_else(|| format!("{z}/{x}/{y} is missing from the converted archive"))?,
            None => rewrite_tile(&before).map_err(|err| format!("rewrite {z}/{x}/{y}: {err}"))?.0,
        };
        bytes_before += before.len() as u64;
        bytes_after += after.len() as u64;
        let mismatches = stats.mismatches.len();
        check_tile((z, x, y), &before, &after, &mut stats)?;
        println!(
            "{z}/{x}/{y}: decoded {} -> {} bytes, {}",
            before.len(),
            after.len(),
            if stats.mismatches.len() == mismatches { "identical" } else { "MISMATCH" }
        );
    }
    for mismatch in &stats.mismatches {
        println!("  {mismatch}");
    }
    println!(
        "{} tiles, {} builds in {:.1}s; decoded {bytes_before} -> {bytes_after} bytes; baked-face hits {} before / {} after; {} mismatches",
        stats.tiles,
        stats.builds,
        start.elapsed().as_secs_f64(),
        stats.baked_hits_before,
        stats.baked_hits_after,
        stats.mismatches.len()
    );
    if stats.mismatches.is_empty() {
        Ok(())
    } else {
        Err("render check failed".to_string())
    }
}

fn parse<T: std::str::FromStr>(value: Option<&String>, flag: &str) -> Result<T, String>
where
    T::Err: std::fmt::Display,
{
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    value
        .parse::<T>()
        .map_err(|err| format!("invalid {flag} value '{value}': {err}"))
}

fn main() {
    if let Err(error) = run() {
        eprintln!("makepad-map-render-check: {error}");
        std::process::exit(1);
    }
}
