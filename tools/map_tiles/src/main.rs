mod bridge_bake;
mod merge;
mod nav_build;
mod native;
mod pbf_audit;
mod versatiles;

use flate2::write::GzEncoder;
use flate2::Compression;
use makepad_fast_inflate::gzip_decompress_vec;
use makepad_mbtile_reader::{MbtilesReader, MbtilesWriter};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use versatiles::{decompress_tile, GeoBounds, TileCompression, VersaTilesReader};

const USAGE: &str = "\
Usage:
  makepad-map-tiles versatiles <source.versatiles> <output.mbtiles> [options]
  makepad-map-tiles pbf-detail <source.osm.pbf> <output.mbtiles> --store <directory> [options]
  makepad-map-tiles inspect-pbf <source.osm.pbf>
  makepad-map-tiles audit-pbf <source.osm.pbf>
  makepad-map-tiles probe-mbtiles <source.mbtiles> <z/x/y>
  makepad-map-tiles nav-build <source.osm.pbf> <basename> [--bbox w,s,e,n] [--skip-addresses]
  makepad-map-tiles nav-probe <basename> search <query...> [--near lon,lat]
  makepad-map-tiles nav-probe <basename> route <lon,lat> <lon,lat> [--mode car|bike|foot]
  makepad-map-tiles bridge-bake <detail.mbtiles> <output.mbtiles> --bbox w,s,e,n [--ahn DIR] [--base base.mbtiles] [--zoom 14]

The legacy form without the 'versatiles' command is also accepted.

Nav artifacts: nav-build writes <basename>.graph (routing graph) and
<basename>.search (place/POI/street index) from one PBF scan.

VersaTiles options:
  --bbox west,south,east,north  Geographic extract (defaults to Geofabrik Europe)
  --planet                       Copy the entire archive
  --max-zoom N                   Stop below the source maximum zoom
  --plan-only                    Inspect indexes and report selection without writing
  --force                        Replace an existing output file

PBF detail options:
  --store DIRECTORY              Required bounded-memory scratch directory
  --zoom N                       Detail tile zoom (default: 14)
  --sort-memory-mib N            Per-block external-sort memory (default: 256)

General:
  -h, --help                     Show this help
";

#[derive(Debug)]
struct Options {
    source: PathBuf,
    output: PathBuf,
    bounds: Option<GeoBounds>,
    max_zoom: Option<u8>,
    plan_only: bool,
    force: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct SelectionPlan {
    blocks: usize,
    tiles: u64,
    source_tile_bytes: u64,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("makepad-map-tiles: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print!("{USAGE}");
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "audit-pbf") {
        if args.len() != 2 {
            return Err(USAGE.to_string());
        }
        return pbf_audit::audit(Path::new(&args[1]));
    }
    if args.first().is_some_and(|arg| arg == "inspect-pbf") {
        if args.len() != 2 {
            return Err(USAGE.to_string());
        }
        return pbf_audit::inspect_header(Path::new(&args[1]));
    }
    if args.first().is_some_and(|arg| arg == "probe-mbtiles") {
        if args.len() != 3 {
            return Err(USAGE.to_string());
        }
        return probe_mbtiles(Path::new(&args[1]), &args[2]);
    }
    if args.first().is_some_and(|arg| arg == "pbf-detail") {
        return native::convert_detail(parse_detail_options(&args)?);
    }
    if args.first().is_some_and(|arg| arg == "bridge-bake") {
        return bridge_bake::bake(bridge_bake::parse_bake_options(&args)?);
    }
    // mbtiles-merge <in1> <in2> [...] <output> [--zoom 14] — later inputs win
    if args.first().is_some_and(|arg| arg == "mbtiles-merge") {
        let mut zoom = 14u8;
        let mut paths = Vec::new();
        let mut iter = args[1..].iter();
        while let Some(arg) = iter.next() {
            if arg == "--zoom" {
                zoom = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--zoom needs a number")?;
            } else {
                paths.push(arg.clone());
            }
        }
        let output = paths.pop().ok_or("mbtiles-merge needs inputs and an output")?;
        return merge::merge(&paths, &output, zoom);
    }
    if args.first().is_some_and(|arg| arg == "nav-build") {
        return nav_build::nav_build(nav_build::parse_nav_build_options(&args)?);
    }
    if args.first().is_some_and(|arg| arg == "nav-probe") {
        return nav_build::nav_probe(&args);
    }
    if args.first().is_some_and(|arg| arg == "versatiles") {
        args.remove(0);
    }
    let options = parse_options(args)?;
    convert(options)
}

fn parse_detail_options(args: &[String]) -> Result<native::DetailOptions, String> {
    if args.len() < 3 {
        return Err(USAGE.to_string());
    }
    let source = PathBuf::from(&args[1]);
    let output = PathBuf::from(&args[2]);
    let mut store = None;
    let mut zoom = None;
    let mut sort_memory_mib = None;
    let mut index = 3;
    while index < args.len() {
        match args[index].as_str() {
            "--store" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--store requires a directory".to_string())?;
                if store.replace(PathBuf::from(value)).is_some() {
                    return Err("--store may only be specified once".to_string());
                }
                index += 2;
            }
            "--zoom" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--zoom requires a number".to_string())?;
                zoom = Some(
                    value
                        .parse::<u8>()
                        .map_err(|err| format!("invalid --zoom '{value}': {err}"))?,
                );
                index += 2;
            }
            "--sort-memory-mib" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--sort-memory-mib requires a number".to_string())?;
                sort_memory_mib =
                    Some(value.parse::<usize>().map_err(|err| {
                        format!("invalid --sort-memory-mib '{value}': {err}")
                    })?);
                index += 2;
            }
            value => return Err(format!("unknown pbf-detail argument '{value}'\n\n{USAGE}")),
        }
    }
    let store = store.ok_or_else(|| "pbf-detail requires --store DIRECTORY".to_string())?;
    let mut options = native::default_detail_options(source, output, store);
    if let Some(zoom) = zoom {
        options.zoom = zoom;
    }
    if let Some(sort_memory_mib) = sort_memory_mib {
        options.sort_memory_mib = sort_memory_mib;
    }
    Ok(options)
}

fn probe_mbtiles(path: &Path, tile: &str) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    let parts = tile
        .split('/')
        .map(|part| {
            part.parse::<u32>()
                .map_err(|err| format!("invalid XYZ tile '{tile}': {err}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if parts.len() != 3 {
        return Err(format!("invalid XYZ tile '{tile}'; expected z/x/y"));
    }
    let zoom =
        u8::try_from(parts[0]).map_err(|_| format!("tile zoom {} is too large", parts[0]))?;
    let x = parts[1];
    let y = parts[2];
    let axis = 1_u32
        .checked_shl(u32::from(zoom))
        .ok_or_else(|| format!("tile zoom {zoom} is too large"))?;
    if x >= axis || y >= axis {
        return Err(format!("XYZ tile z{zoom}/{x}/{y} is outside its pyramid"));
    }
    let tms_y = axis - 1 - y;

    let started = Instant::now();
    let mut reader =
        MbtilesReader::open(path).map_err(|err| format!("open {}: {err}", path.display()))?;
    let direct = reader.supports_direct_tile_lookup();
    if !direct {
        return Err(format!(
            "{} has neither Makepad rowids nor a standard tile index; direct streaming is unavailable",
            path.display()
        ));
    }
    let payload = reader
        .get_tile(i64::from(zoom), i64::from(x), i64::from(tms_y))
        .map_err(|err| format!("read z{zoom}/{x}/{y}: {err}"))?
        .ok_or_else(|| format!("{} has no tile z{zoom}/{x}/{y}", path.display()))?;
    println!("source={}", path.display());
    println!("tile={zoom}/{x}/{y}");
    println!("direct_lookup={direct}");
    println!("tile_bytes={}", payload.len());
    let gzip = payload.starts_with(&[0x1f, 0x8b]);
    println!("compression={}", if gzip { "gzip" } else { "uncompressed" });
    let pbf = if gzip {
        gzip_decompress_vec(&payload)
            .map_err(|err| format!("decompress z{zoom}/{x}/{y}: {err}"))?
    } else {
        payload
    };
    let inspected = native::inspect_mvt_tile(&pbf)
        .map_err(|err| format!("inspect MVT z{zoom}/{x}/{y}: {err}"))?;
    let mut feature_total = 0_u64;
    let mut tag_features = BTreeMap::<String, u64>::new();
    for layer in inspected.layers {
        println!("layer[{}]_features={}", layer.name, layer.features);
        feature_total += layer.features;
        for (key, count) in layer.tag_features {
            *tag_features.entry(key).or_default() += count;
        }
    }
    println!("mvt_features={feature_total}");
    for key in [
        "amenity",
        "leisure",
        "building",
        "building:part",
        "height",
        "min_height",
        "building:levels",
        "building:min_level",
        "roof:shape",
        "roof:height",
        "roof:levels",
        "roof:direction",
        "roof:orientation",
        "building:material",
        "building:colour",
        "roof:material",
        "roof:colour",
    ] {
        if let Some(count) = tag_features.get(key) {
            println!("tag[{key}]_features={count}");
        }
    }
    println!("elapsed_milliseconds={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    Ok(())
}

fn convert(options: Options) -> Result<(), String> {
    assert_paths_are_files(&options.source, &options.output)?;
    if !options.plan_only && options.output.exists() && !options.force {
        return Err(format!(
            "{} already exists; pass --force to replace it",
            options.output.display()
        ));
    }
    if !options.plan_only && options.source == options.output {
        return Err("source and output paths must differ".to_string());
    }

    let started = Instant::now();
    println!("Opening {}", options.source.display());
    let mut source = VersaTilesReader::open(&options.source)?;
    let max_zoom = options
        .max_zoom
        .unwrap_or(source.header.max_zoom)
        .min(source.header.max_zoom);
    if max_zoom < source.header.min_zoom {
        return Err(format!(
            "--max-zoom {max_zoom} is below source minimum {}",
            source.header.min_zoom
        ));
    }

    let output_bounds = options.bounds.unwrap_or(source.header.bounds);
    let bounds_per_zoom = (0..=max_zoom)
        .map(|zoom| output_bounds.tile_bounds(zoom))
        .collect::<Vec<_>>();
    let blocks = source.blocks.clone();
    let plan = plan_selection(&mut source, &blocks, &bounds_per_zoom, max_zoom)?;

    println!(
        "Source: Shortbread MVT, {:?}, z{}-{} ({} blocks)",
        source.header.compression,
        source.header.min_zoom,
        source.header.max_zoom,
        source.blocks.len()
    );
    println!(
        "Extract: {} through z{} ({} blocks, {} tiles, {:.2} GiB source payload)",
        output_bounds.as_csv(),
        max_zoom,
        plan.blocks,
        plan.tiles,
        plan.source_tile_bytes as f64 / 1_073_741_824.0
    );

    if options.plan_only {
        println!("Plan only: no output was created");
        return Ok(());
    }

    if options.force && options.output.exists() {
        fs::remove_file(&options.output)
            .map_err(|err| format!("remove {}: {err}", options.output.display()))?;
    }
    if let Some(parent) = options.output.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create {}: {err}", parent.display()))?;
    }

    let mut writer = MbtilesWriter::create(&options.output)
        .map_err(|err| format!("create {}: {err}", options.output.display()))?;
    add_metadata(
        &mut writer,
        &source.metadata_json,
        output_bounds,
        source.header.min_zoom,
        max_zoom,
    )?;

    let mut tile_reader = source.open_tile_reader(&options.source)?;
    let mut tile_count = 0_u64;
    let mut tile_bytes = 0_u64;
    let mut selected_block_index = 0_usize;
    let mut last_progress = Instant::now();
    // The brotli-decode + gzip-encode transcode pegs one core; the source
    // reads are sequential I/O and the MBTiles writer needs block-major
    // rowid order, so parallelize per block: read the block's tiles, fan
    // the transcode across cores, then write in order.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .saturating_sub(1)
        .max(1);
    // Reused across blocks to keep allocations stable.
    let mut pending: Vec<(u32, u32, Vec<u8>)> = Vec::new();

    for block in &blocks {
        if block.zoom > max_zoom {
            continue;
        }
        let selected = bounds_per_zoom[block.zoom as usize];
        if !selected.intersects(block.bounds()) {
            continue;
        }
        selected_block_index += 1;

        let tile_index = source.read_tile_index(block)?;
        let width = block.x_max - block.x_min + 1;
        pending.clear();
        for (index, range) in tile_index.into_iter().enumerate() {
            if range.length == 0 {
                continue;
            }
            let index = index as u32;
            let x = block.x_min + index % width;
            let y = block.y_min + index / width;
            if !selected.contains(x, y) {
                continue;
            }
            let source_tile = tile_reader.read(range.offset, range.length).map_err(|err| {
                format!(
                    "read tile z{}/{}/{} at {}+{}: {err}",
                    block.zoom, x, y, range.offset, range.length
                )
            })?;
            pending.push((x, y, source_tile));
        }

        let compression = source.header.compression;
        let zoom = block.zoom;
        let chunk_size = pending.len().div_ceil(workers).max(1);
        let mut transcoded: Vec<Result<Vec<u8>, String>> = Vec::with_capacity(pending.len());
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for chunk in pending.chunks(chunk_size) {
                handles.push(scope.spawn(move || {
                    chunk
                        .iter()
                        .map(|(x, y, source_tile)| {
                            mbtiles_pbf(source_tile, compression).map_err(|err| {
                                format!("transcode tile z{}/{}/{} to gzip: {err}", zoom, x, y)
                            })
                        })
                        .collect::<Vec<_>>()
                }));
            }
            for handle in handles {
                transcoded.extend(handle.join().expect("transcode worker panicked"));
            }
        });

        for ((x, y, _), tile) in pending.iter().zip(transcoded) {
            let tile = tile?;
            writer
                .write_tile_xyz(zoom, *x, *y, &tile)
                .map_err(|err| format!("write tile z{}/{}/{}: {err}", zoom, x, y))?;
            tile_count += 1;
            tile_bytes += tile.len() as u64;

            if last_progress.elapsed() >= Duration::from_secs(2) {
                println!(
                    "  {:>8} tiles, {:>8.2} GiB, z{}, block {}/{} ({:.1}%)",
                    tile_count,
                    tile_bytes as f64 / 1_073_741_824.0,
                    zoom,
                    selected_block_index,
                    plan.blocks,
                    tile_count as f64 * 100.0 / plan.tiles as f64
                );
                last_progress = Instant::now();
            }
        }
    }

    let stats = writer
        .finish()
        .map_err(|err| format!("finalize {}: {err}", options.output.display()))?;
    if stats.tile_count != plan.tiles {
        return Err(format!(
            "selection changed while converting: planned {} tiles, wrote {}",
            plan.tiles, stats.tile_count
        ));
    }
    println!(
        "Done: {} tiles, {:.2} GiB payload, {:.2} GiB file in {:.1}s",
        stats.tile_count,
        stats.tile_bytes as f64 / 1_073_741_824.0,
        stats.file_bytes as f64 / 1_073_741_824.0,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

fn plan_selection(
    source: &mut VersaTilesReader,
    blocks: &[versatiles::BlockDefinition],
    bounds_per_zoom: &[versatiles::TileBounds],
    max_zoom: u8,
) -> Result<SelectionPlan, String> {
    let mut plan = SelectionPlan::default();
    for block in blocks {
        if block.zoom > max_zoom {
            continue;
        }
        let selected = bounds_per_zoom[block.zoom as usize];
        if !selected.intersects(block.bounds()) {
            continue;
        }
        plan.blocks += 1;
        let width = block.x_max - block.x_min + 1;
        for (index, range) in source.read_tile_index(block)?.into_iter().enumerate() {
            if range.length == 0 {
                continue;
            }
            let index = index as u32;
            let x = block.x_min + index % width;
            let y = block.y_min + index / width;
            if !selected.contains(x, y) {
                continue;
            }
            plan.tiles += 1;
            plan.source_tile_bytes = plan
                .source_tile_bytes
                .checked_add(u64::from(range.length))
                .ok_or_else(|| "selected source tile byte count overflow".to_string())?;
        }
    }
    if plan.tiles == 0 {
        return Err("the requested bounds contain no source tiles".to_string());
    }
    Ok(plan)
}

fn mbtiles_pbf(source: &[u8], compression: TileCompression) -> Result<Vec<u8>, String> {
    if compression == TileCompression::Gzip {
        return Ok(source.to_vec());
    }
    let pbf = decompress_tile(source, compression)?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder
        .write_all(&pbf)
        .map_err(|err| format!("write gzip stream: {err}"))?;
    encoder
        .finish()
        .map_err(|err| format!("finish gzip stream: {err}"))
}

fn parse_options(args: Vec<String>) -> Result<Options, String> {
    if args.len() < 2 {
        return Err(USAGE.to_string());
    }

    let source = PathBuf::from(&args[0]);
    let output = PathBuf::from(&args[1]);
    let mut bounds = Some(GeoBounds::EUROPE);
    let mut max_zoom = None;
    let mut plan_only = false;
    let mut force = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--bbox" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--bbox requires west,south,east,north".to_string())?;
                bounds = Some(GeoBounds::parse(value)?);
                index += 2;
            }
            "--planet" => {
                bounds = None;
                index += 1;
            }
            "--max-zoom" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--max-zoom requires a number".to_string())?;
                max_zoom = Some(
                    value
                        .parse::<u8>()
                        .map_err(|err| format!("invalid --max-zoom '{value}': {err}"))?,
                );
                index += 2;
            }
            "--force" => {
                force = true;
                index += 1;
            }
            "--plan-only" => {
                plan_only = true;
                index += 1;
            }
            value => return Err(format!("unknown argument '{value}'\n\n{USAGE}")),
        }
    }

    Ok(Options {
        source,
        output,
        bounds,
        max_zoom,
        plan_only,
        force,
    })
}

fn add_metadata(
    writer: &mut MbtilesWriter,
    source_json: &[u8],
    bounds: GeoBounds,
    min_zoom: u8,
    max_zoom: u8,
) -> Result<(), String> {
    let source = if source_json.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(source_json)
            .map_err(|err| format!("parse source TileJSON metadata: {err}"))?
    };
    let source_object = source.as_object();

    writer.set_metadata(
        "name",
        json_string(source_object, "name").unwrap_or("Shortbread Europe"),
    );
    writer.set_metadata(
        "description",
        json_string(source_object, "description")
            .unwrap_or("OpenStreetMap vector tiles in the Shortbread schema"),
    );
    writer.set_metadata("type", "baselayer");
    writer.set_metadata("version", "1.0");
    writer.set_metadata("format", "pbf");
    writer.set_metadata("scheme", "tms");
    writer.set_metadata("minzoom", min_zoom.to_string());
    writer.set_metadata("maxzoom", max_zoom.to_string());
    writer.set_metadata("bounds", bounds.as_csv());
    let (center_lon, center_lat) = bounds.center();
    writer.set_metadata(
        "center",
        format!("{center_lon:.7},{center_lat:.7},{}", max_zoom.min(7)),
    );
    writer.set_metadata("license", "Open Database License 1.0");
    if let Some(attribution) = json_string(source_object, "attribution") {
        writer.set_metadata("attribution", attribution);
        writer.set_metadata("author", attribution);
    } else {
        writer.set_metadata("attribution", "OpenStreetMap contributors");
        writer.set_metadata("author", "OpenStreetMap contributors");
    }

    let mut json_metadata = Map::new();
    if let Some(object) = source_object {
        for key in ["vector_layers", "tilestats"] {
            if let Some(value) = object.get(key) {
                json_metadata.insert(key.to_string(), value.clone());
            }
        }
    }
    json_metadata
        .entry("vector_layers")
        .or_insert_with(|| Value::Array(Vec::new()));
    writer.set_metadata(
        "json",
        serde_json::to_string(&json_metadata)
            .map_err(|err| format!("serialize MBTiles JSON metadata: {err}"))?,
    );
    Ok(())
}

fn json_string<'a>(object: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    object?.get(key)?.as_str()
}

fn assert_paths_are_files(source: &Path, output: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("{} is not a file", source.display()));
    }
    if output.is_dir() {
        return Err(format!("{} is a directory", output.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brotli::CompressorWriter;
    use makepad_fast_inflate::gzip_decompress_vec;
    use makepad_mbtile_reader::MbtilesReader;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(extension: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        env::temp_dir().join(format!(
            "makepad-map-tiles-{}-{nonce}.{extension}",
            std::process::id()
        ))
    }

    fn brotli_compress(bytes: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        {
            let mut writer = CompressorWriter::new(&mut output, 4096, 5, 22);
            writer.write_all(bytes).unwrap();
        }
        output
    }

    fn make_source(path: &Path) -> Vec<Vec<u8>> {
        let tiles = vec![
            vec![0x1f, 0x8b, 0, 1],
            vec![0x1f, 0x8b, 2, 3, 4],
            vec![0x1f, 0x8b, 5],
            vec![0x1f, 0x8b, 6, 7, 8, 9],
        ];
        let tiles_offset = 66_u64;
        let mut tile_bytes = Vec::new();
        let mut tile_index = Vec::new();
        for tile in &tiles {
            tile_index.extend_from_slice(&(tile_bytes.len() as u64).to_be_bytes());
            tile_index.extend_from_slice(&(tile.len() as u32).to_be_bytes());
            tile_bytes.extend_from_slice(tile);
        }
        let compressed_tile_index = brotli_compress(&tile_index);
        let index_offset = tiles_offset + tile_bytes.len() as u64;

        let mut block = Vec::new();
        block.push(1);
        block.extend_from_slice(&0_u32.to_be_bytes());
        block.extend_from_slice(&0_u32.to_be_bytes());
        block.extend_from_slice(&[0, 0, 1, 1]);
        block.extend_from_slice(&tiles_offset.to_be_bytes());
        block.extend_from_slice(&(tile_bytes.len() as u64).to_be_bytes());
        block.extend_from_slice(&(compressed_tile_index.len() as u32).to_be_bytes());
        assert_eq!(block.len(), 33);
        let compressed_block_index = brotli_compress(&block);
        let blocks_offset = index_offset + compressed_tile_index.len() as u64;

        let mut header = [0_u8; 66];
        header[0..14].copy_from_slice(b"versatiles_v02");
        header[14] = 0x20;
        header[15] = 1;
        header[16] = 1;
        header[17] = 1;
        header[18..22].copy_from_slice(&(-1_800_000_000_i32).to_be_bytes());
        header[22..26].copy_from_slice(&(-850_511_287_i32).to_be_bytes());
        header[26..30].copy_from_slice(&(1_800_000_000_i32).to_be_bytes());
        header[30..34].copy_from_slice(&(850_511_287_i32).to_be_bytes());
        header[34..42].copy_from_slice(&0_u64.to_be_bytes());
        header[42..50].copy_from_slice(&0_u64.to_be_bytes());
        header[50..58].copy_from_slice(&blocks_offset.to_be_bytes());
        header[58..66].copy_from_slice(&(compressed_block_index.len() as u64).to_be_bytes());

        let mut source = header.to_vec();
        source.extend_from_slice(&tile_bytes);
        source.extend_from_slice(&compressed_tile_index);
        source.extend_from_slice(&compressed_block_index);
        fs::write(path, source).unwrap();
        tiles
    }

    #[test]
    fn converts_versatiles_to_mbtiles_end_to_end() {
        let source_path = temp_path("versatiles");
        let output_path = temp_path("mbtiles");
        let tiles = make_source(&source_path);

        convert(Options {
            source: source_path.clone(),
            output: output_path.clone(),
            bounds: None,
            max_zoom: None,
            plan_only: false,
            force: false,
        })
        .unwrap();

        let mut reader = MbtilesReader::open(&output_path).unwrap();
        assert_eq!(reader.tile_summary().unwrap(), vec![(1, 4)]);
        assert_eq!(reader.get_tile(1, 0, 1).unwrap().unwrap(), tiles[0]);
        assert_eq!(reader.get_tile(1, 1, 1).unwrap().unwrap(), tiles[1]);
        assert_eq!(reader.get_tile(1, 0, 0).unwrap().unwrap(), tiles[2]);
        assert_eq!(reader.get_tile(1, 1, 0).unwrap().unwrap(), tiles[3]);

        if env::var_os("MAKEPAD_KEEP_MAP_TILE_TEST").is_some() {
            println!("kept source: {}", source_path.display());
            println!("kept output: {}", output_path.display());
        } else {
            fs::remove_file(source_path).unwrap();
            fs::remove_file(output_path).unwrap();
        }
    }

    #[test]
    fn transcodes_brotli_and_raw_tiles_to_gzip() {
        let pbf = (0..65_537)
            .map(|index| ((index * 31) % 251) as u8)
            .collect::<Vec<_>>();
        for (source, compression) in [
            (brotli_compress(&pbf), TileCompression::Brotli),
            (pbf.clone(), TileCompression::Uncompressed),
        ] {
            let gzip = mbtiles_pbf(&source, compression).unwrap();
            assert_eq!(&gzip[..2], &[0x1f, 0x8b]);
            assert_eq!(gzip_decompress_vec(&gzip).unwrap(), pbf);
        }
    }
}
