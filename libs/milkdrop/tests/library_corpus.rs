//! The library over a whole preset pack, run only when `MILKDROP_CORPUS`
//! names a directory of `.milk` files (a downloaded pack). Prints the index
//! size, category counts, cold and warm walk times and search latency.

use makepad_milkdrop::library::{scan_cached, Filter, UserPresets};
use std::path::PathBuf;
use std::time::Instant;

#[test]
fn library_over_the_corpus() {
    let Some(root) = std::env::var_os("MILKDROP_CORPUS").map(PathBuf::from) else { return };
    let cache = std::env::temp_dir().join(format!("milkdrop-library-corpus-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&cache);
    let (lib, cold) = scan_cached(&root, Some(&cache)).expect("the corpus directory");
    let (again, warm) = scan_cached(&root, Some(&cache)).unwrap();
    assert_eq!(lib.entries, again.entries);
    assert_eq!(warm.read, 0);
    let (md1, md2, shaders) = lib.counts();
    println!("presets {} (MD1 {md1}, MD2 {md2}, with shaders {shaders})", lib.entries.len());
    println!("walk cold {} ms ({} read), warm {} ms ({} reused)", cold.millis, cold.read, warm.millis, warm.reused);
    for node in lib.tree() {
        println!("  {:<14} {:>5}  ({} subcategories)", node.name, node.count, node.subcategories.len());
    }
    let with_authors = lib.entries.iter().filter(|e| !e.authors.is_empty()).count();
    println!("names with authors: {with_authors}");
    let user = UserPresets::default();
    for query in ["flexi", "geiss tunnel", "martin", "x", "suksma nz"] {
        let filter = Filter { query: query.into(), ..Filter::default() };
        let started = Instant::now();
        let mut hits = 0;
        const RUNS: u32 = 20;
        for _ in 0..RUNS {
            hits = lib.filter(&filter, &user).len();
        }
        println!("search {query:?}: {hits} hits, {:.3} ms", started.elapsed().as_secs_f64() * 1000.0 / RUNS as f64);
    }
    let _ = std::fs::remove_file(&cache);
}
