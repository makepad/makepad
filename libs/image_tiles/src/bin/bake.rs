//! The baker CLI: a manifest of image URLs in, a baked tile library out.
//!
//! ```text
//! image-tiles-bake --root <dir> [--fetch N] [--encode N] [--retry-failed] <manifest.tsv>
//! ```
//!
//! The manifest is one picture per line: a bare URL, or
//! `url<TAB>title<TAB>link`. Blank lines and `#` comments are skipped.
//! Re-running is cheap: pictures already baked are skipped. This file is
//! deliberately a thin wrapper — the engine is `makepad_image_tiles::bake`,
//! made to be called from (and customised by) your own tools.

use makepad_image_tiles::bake::{bake, parse_manifest, BakeOptions};
use std::path::PathBuf;

fn usage() -> ! {
    eprintln!("usage: image-tiles-bake --root <dir> [--fetch N] [--encode N] [--retry-failed] <manifest.tsv>");
    std::process::exit(2);
}

fn main() {
    let mut root: Option<PathBuf> = None;
    let mut manifest: Option<PathBuf> = None;
    let mut options = BakeOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().map(PathBuf::from),
            "--fetch" => options.fetch_threads = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--encode" => options.encode_threads = args.next().and_then(|v| v.parse().ok()).unwrap_or_else(|| usage()),
            "--retry-failed" => options.retry_failed = true,
            "--help" | "-h" => usage(),
            _ if arg.starts_with('-') => usage(),
            _ => manifest = Some(PathBuf::from(arg)),
        }
    }
    let Some(manifest) = manifest else { usage() };
    let root = root.unwrap_or_else(|| makepad_image_tiles::Library::resolve().root);
    let text = match std::fs::read_to_string(&manifest) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("read {}: {e}", manifest.display());
            std::process::exit(1);
        }
    };
    let sources = parse_manifest(&text);
    println!("library: {} — {} source(s) in the manifest", root.display(), sources.len());
    match bake(&root, &sources, &options, &mut |line| println!("{line}")) {
        Ok(summary) => {
            if summary.failed > 0 {
                std::process::exit(3);
            }
        }
        Err(e) => {
            eprintln!("bake: {e}");
            std::process::exit(1);
        }
    }
}
