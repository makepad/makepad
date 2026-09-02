use makepad_map_build::repack::{
    repack_archive, repack_status, RepackOptions, ShardRange, TileSelection,
};
use makepad_mbtile_reader::mkmap_tile_id;
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;

const USAGE: &str = "Usage: makepad-map-repack <in.mkmap dir> <out dir> [--tiles <hilbert range | z/x/y list>] [--jobs N] [--brotli-quality Q] [--resume] [--verify [--shards A..B]] [--status] [--log FILE] [--dry-run]\nDefaults: all available cores, Brotli q11, progress log <out>/repack.log; A..B is half-open.";

fn parse_selection(value: &str) -> Result<TileSelection, String> {
    if value.contains('/') {
        let mut ids = BTreeSet::new();
        for item in value
            .split([',', ';', ' '])
            .filter(|item| !item.is_empty())
        {
            let parts: Vec<_> = item.split('/').collect();
            if parts.len() != 3 {
                return Err(format!("invalid tile '{item}', expected z/x/y"));
            }
            let zoom = parts[0]
                .parse::<u8>()
                .map_err(|err| format!("invalid zoom in '{item}': {err}"))?;
            let x = parts[1]
                .parse::<u32>()
                .map_err(|err| format!("invalid x in '{item}': {err}"))?;
            let y = parts[2]
                .parse::<u32>()
                .map_err(|err| format!("invalid y in '{item}': {err}"))?;
            if zoom > 30 || x >= 1_u32 << zoom || y >= 1_u32 << zoom {
                return Err(format!("tile '{item}' is outside its zoom grid"));
            }
            ids.insert(mkmap_tile_id(zoom, x, y));
        }
        if ids.is_empty() {
            return Err("empty z/x/y tile list".to_string());
        }
        return Ok(TileSelection::Explicit(ids));
    }
    let range = value
        .split_once("..=")
        .or_else(|| value.split_once(".."))
        .or_else(|| value.split_once('-'))
        .ok_or_else(|| "Hilbert range must be START-END or START..END".to_string())?;
    let start = range
        .0
        .parse::<u64>()
        .map_err(|err| format!("invalid Hilbert range start '{}': {err}", range.0))?;
    let end = range
        .1
        .parse::<u64>()
        .map_err(|err| format!("invalid Hilbert range end '{}': {err}", range.1))?;
    if start > end {
        return Err("Hilbert range start exceeds end".to_string());
    }
    Ok(TileSelection::HilbertRange { start, end })
}

fn parse_shard_range(value: &str) -> Result<ShardRange, String> {
    let (start, end, inclusive) = if let Some((start, end)) = value.split_once("..=") {
        (start, end, true)
    } else if let Some((start, end)) = value.split_once("..") {
        (start, end, false)
    } else {
        return Err("shard range must be START..END or START..=END".to_string());
    };
    let start = start
        .parse::<usize>()
        .map_err(|err| format!("invalid shard range start '{start}': {err}"))?;
    let mut end = end
        .parse::<usize>()
        .map_err(|err| format!("invalid shard range end '{end}': {err}"))?;
    if inclusive {
        end = end
            .checked_add(1)
            .ok_or_else(|| "inclusive shard range end overflows".to_string())?;
    }
    if start >= end {
        return Err("shard range must not be empty".to_string());
    }
    Ok(ShardRange { start, end })
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.len() < 2 || args[0].starts_with('-') || args[1].starts_with('-') {
        return Err(USAGE.to_string());
    }
    let mut options = RepackOptions {
        input: PathBuf::from(&args[0]),
        output: PathBuf::from(&args[1]),
        selection: TileSelection::All,
        dry_run: false,
        verify: false,
        resume: false,
        jobs: std::thread::available_parallelism()
            .map(|parallelism| parallelism.get())
            .unwrap_or(1),
        brotli_quality: 11,
        verify_shards: None,
        log: None,
    };
    let mut status = false;
    let mut explicit_log = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--tiles" => {
                let value = args.get(index + 1).ok_or("--tiles requires a value")?;
                options.selection = parse_selection(value)?;
                index += 2;
            }
            "--dry-run" => {
                options.dry_run = true;
                index += 1;
            }
            "--verify" => {
                options.verify = true;
                index += 1;
            }
            "--resume" => {
                options.resume = true;
                index += 1;
            }
            "--jobs" => {
                let value = args.get(index + 1).ok_or("--jobs requires a value")?;
                options.jobs = value
                    .parse::<usize>()
                    .map_err(|err| format!("invalid --jobs value '{value}': {err}"))?;
                if options.jobs == 0 {
                    return Err("--jobs must be at least 1".to_string());
                }
                index += 2;
            }
            "--brotli-quality" => {
                let value = args
                    .get(index + 1)
                    .ok_or("--brotli-quality requires a value")?;
                options.brotli_quality = value.parse::<u32>().map_err(|err| {
                    format!("invalid --brotli-quality value '{value}': {err}")
                })?;
                if options.brotli_quality > 11 {
                    return Err("--brotli-quality must be in 0..=11".to_string());
                }
                index += 2;
            }
            "--shards" => {
                let value = args.get(index + 1).ok_or("--shards requires a value")?;
                options.verify_shards = Some(parse_shard_range(value)?);
                index += 2;
            }
            "--status" => {
                status = true;
                index += 1;
            }
            "--log" => {
                let value = args.get(index + 1).ok_or("--log requires a value")?;
                options.log = Some(PathBuf::from(value));
                explicit_log = true;
                index += 2;
            }
            other => return Err(format!("unknown argument '{other}'\n{USAGE}")),
        }
    }
    if options.verify_shards.is_some() && !options.verify {
        return Err("--shards requires --verify".to_string());
    }
    if status {
        if options.dry_run || options.verify || options.resume || explicit_log {
            return Err("--status cannot be combined with --dry-run, --verify, --resume, or --log"
                .to_string());
        }
        let status = repack_status(&options)?;
        println!(
            "{}/{} done, {} bytes in, {} bytes out",
            status.completed_shards,
            status.total_shards,
            status.compressed_before,
            status.compressed_after
        );
        return Ok(());
    }
    if !options.dry_run && options.log.is_none() {
        options.log = Some(options.output.join("repack.log"));
    }
    repack_archive(&options).map(|_| ())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("makepad-map-repack: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tile_selection_forms() {
        assert!(matches!(
            parse_selection("10..20").unwrap(),
            TileSelection::HilbertRange { start: 10, end: 20 }
        ));
        let TileSelection::Explicit(ids) = parse_selection("14/8412/5382,14/8416/5386").unwrap()
        else {
            panic!("explicit selection expected")
        };
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn parses_half_open_and_inclusive_shard_ranges() {
        assert_eq!(
            parse_shard_range("2..5").unwrap(),
            ShardRange { start: 2, end: 5 }
        );
        assert_eq!(
            parse_shard_range("2..=5").unwrap(),
            ShardRange { start: 2, end: 6 }
        );
    }
}
