//! Hashes the pinned files of registry models that are already in a hub's
//! cache and writes their verification receipts, so the hub can serve them
//! (peer transfers, paired phones) without ever downloading. Nothing is
//! fetched: a missing or mismatching file is reported and left alone.
//!
//!     hub-verify-files <cache-dir> <registry.json> <model-id>...

use makepad_ai_hub::backend::CancelToken;
use makepad_ai_hub::download::{source_file_is_verified, Downloader};
use makepad_ai_hub::registry::Registry;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: hub-verify-files <cache-dir> <registry.json> <model-id>...");
        std::process::exit(2);
    }
    let cache = PathBuf::from(&args[0]);
    let registry = Registry::load_file(&PathBuf::from(&args[1])).unwrap_or_else(|e| {
        eprintln!("registry: {e}");
        std::process::exit(2);
    });
    // No network: a file that is absent or does not verify fails here
    // instead of being fetched.
    let downloader = Downloader::new("http://127.0.0.1:9", None).expect("downloader");
    let mut failed = 0;
    for id in &args[2..] {
        let Some(model) = registry.find(id) else {
            eprintln!("{id}: not in the registry");
            failed += 1;
            continue;
        };
        for file in &model.files {
            if file.sha256.is_none() || file.size.is_none() {
                println!("{id}: {} has no pinned sha256/size, skipped", file.cache_as);
                continue;
            }
            let dest = file.dest_path(&cache);
            if !dest.is_file() {
                println!("{id}: {} MISSING", dest.display());
                failed += 1;
                continue;
            }
            match downloader.ensure_file(file, &cache, &mut |_| {}, &CancelToken::new()) {
                Ok(_) if source_file_is_verified(file, &cache) => println!("{id}: {} verified", file.cache_as),
                Ok(_) => {
                    println!("{id}: {} not verified", file.cache_as);
                    failed += 1;
                }
                Err(e) => {
                    println!("{id}: {} FAILED: {e}", file.cache_as);
                    failed += 1;
                }
            }
        }
    }
    std::process::exit(if failed == 0 { 0 } else { 1 });
}
