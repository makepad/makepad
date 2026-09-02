use makepad_map_build::repack::{repack_archive, RepackOptions, TileSelection};
use makepad_mbtile_reader::mkmap_tile_id;
use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;

const USAGE: &str = "Usage: makepad-map-repack <in.mkmap dir> <out dir> [--tiles <hilbert range | z/x/y list>] [--dry-run] [--verify] [--resume]";

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
    };
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
            other => return Err(format!("unknown argument '{other}'\n{USAGE}")),
        }
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
}
