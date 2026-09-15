//! `makepad-map-bake` — the painter-cascade face baker, from a shell.
//!
//! The pass itself is `makepad_map_build::faces`, shared with the test-map
//! recipe an app runs on its own worker thread. This is argument parsing
//! and nothing else.
//!
//! Usage:
//!   makepad-map-bake <in.mbtiles> <out.mbtiles> [--bridge-dz <dz.mbtiles>]
//!       [--brotli-quality N] [--limit N] [--recompress] [--fingerprint]
//!       [--threshold-ms N] [--buckets a,b,c] [--zooms a,b,c] [--full]

use makepad_map_build::faces::{bake_faces, default_face_bake_options, fingerprint};
use std::path::{Path, PathBuf};

fn main() {
    if let Err(err) = run() {
        eprintln!("makepad-map-bake: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        return Err(format!(
            "usage: {} <in.mbtiles> <out.mbtiles> [--bridge-dz dz.mbtiles] \
             [--brotli-quality N] [--limit N] [--recompress] [--fingerprint] \
             [--threshold-ms N] [--buckets a,b,c] [--zooms a,b,c] [--full]",
            args[0]
        ));
    }
    let mut options =
        default_face_bake_options(PathBuf::from(&args[1]), PathBuf::from(&args[2]));
    let mut fingerprint_only = false;
    let mut i = 3;
    while i < args.len() {
        let value = |i: usize| -> Result<&String, String> {
            args.get(i + 1).ok_or_else(|| format!("{} needs a value", args[i]))
        };
        match args[i].as_str() {
            "--bridge-dz" => {
                options.bridge_dz = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--recompress" => {
                options.recompress = true;
                i += 1;
            }
            "--full" => {
                options.full = true;
                i += 1;
            }
            "--brotli-quality" => {
                options.brotli_quality = value(i)?.parse().map_err(|_| "bad quality")?;
                i += 2;
            }
            "--limit" => {
                options.limit = value(i)?.parse().map_err(|_| "bad limit")?;
                i += 2;
            }
            "--fingerprint" => {
                fingerprint_only = true;
                i += 1;
            }
            "--threshold-ms" => {
                options.threshold_ms = value(i)?.parse().map_err(|_| "bad threshold")?;
                i += 2;
            }
            "--buckets" => {
                options.buckets = parse_list(value(i)?, "bucket")?;
                i += 2;
            }
            "--zooms" => {
                options.zooms = parse_list(value(i)?, "zoom")?;
                i += 2;
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }

    if fingerprint_only {
        let value = fingerprint(Path::new(&args[1]))?;
        println!("MAPBAKE-FINGERPRINT {value:016x}");
        return Ok(());
    }
    let stats = bake_faces(&options)?;
    println!(
        "done: {} tiles, {} baked, {} copied, {} skipped, {:.2} GiB file",
        stats.total,
        stats.baked,
        stats.copied,
        stats.skipped.len(),
        stats.file_bytes as f64 / 1_073_741_824.0
    );
    Ok(())
}

fn parse_list(value: &str, what: &str) -> Result<Vec<u32>, String> {
    value
        .split(',')
        .map(|item| {
            item.trim()
                .parse()
                .map_err(|_| format!("bad {what} {}", item.trim()))
        })
        .collect()
}
