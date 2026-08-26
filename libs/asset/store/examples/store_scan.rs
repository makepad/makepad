//! Open a server root read-mostly and prove it is healthy: schema version,
//! CAS hash-path fan-out, browse paging, alias resolution, manifest decode
//! and verified blob reads.
//!
//! Opening a root runs the schema migration, so this is also how a v7 root
//! (one-level CAS paths, no scale indices) is migrated and checked. Point it
//! at a COPY of a live root, never at one a server is using.
//!
//!   cargo run --release -p makepad-asset-store --example store_scan -- <root>

use makepad_asset_data::{AssetAlias, AssetManifest};
use makepad_asset_store::{
    AssetServerCore, Budgets, SearchFilters, SearchQuery, SearchViewer, ViewerScope,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let root = match std::env::args().nth(1) {
        Some(r) => PathBuf::from(r),
        None => {
            eprintln!("usage: store_scan <server-root>");
            std::process::exit(2);
        }
    };
    let t0 = Instant::now();
    let core = AssetServerCore::open(&root, Budgets::default_v1()).expect("open root");
    println!("open + migrate: {:?}", t0.elapsed());
    println!(
        "schema user_version: {}",
        makepad_asset_store::SERVER_SCHEMA_VERSION
    );

    cas_layout(&root.join("cas/objects"));

    // Browse the catalog through the same keyset paging the routes use.
    let viewer = SearchViewer { principal: None, scope: ViewerScope::All };
    let query =
        SearchQuery { text: "", filters: SearchFilters::default(), page_size: 100, facets: 0 };
    let t = Instant::now();
    let page = core.search().search(&query, &viewer, None).expect("browse page 1");
    println!(
        "browse page 1: {} hits of {} total in {:?}",
        page.hits.len(),
        page.total,
        t.elapsed()
    );
    let t = Instant::now();
    let mut walked = page.hits.len();
    let mut cursor = page.cursor.clone();
    let mut pages = 1;
    while let Some(c) = cursor {
        let next = core.search().search(&query, &viewer, Some(&c)).expect("browse page n");
        walked += next.hits.len();
        pages += 1;
        cursor = next.cursor;
        if pages >= 40 {
            break;
        }
    }
    println!("walked {walked} hits over {pages} keyset pages in {:?}", t.elapsed());

    // Resolve aliases and read every blob of the first few assets, verified.
    let mut resolved = 0;
    let mut blobs = 0;
    let mut bytes = 0u64;
    let t = Instant::now();
    for hit in page.hits.iter().take(25) {
        let Some(alias) = hit.alias.as_deref() else { continue };
        let alias = AssetAlias::new(alias).expect("stored alias parses");
        let target = core
            .catalog()
            .resolve_asset_alias(&alias)
            .expect("resolve")
            .expect("alias head exists");
        resolved += 1;
        let manifest_bytes = core
            .catalog()
            .asset_revision_manifest(&target.revision)
            .expect("manifest read")
            .expect("head revision stored");
        let manifest = AssetManifest::from_canonical_bytes(&manifest_bytes).expect("manifest");
        for f in &manifest.files {
            let data = core.read_blob(&f.blob).expect("verified blob read");
            assert_eq!(data.len() as u64, f.byte_len, "declared byte_len");
            blobs += 1;
            bytes += data.len() as u64;
        }
        if let Some(t) = &manifest.thumbnail {
            let data = core.read_blob(&t.blob).expect("verified thumbnail read");
            blobs += 1;
            bytes += data.len() as u64;
        }
    }
    println!(
        "resolved {resolved} aliases, read+verified {blobs} blobs ({bytes} bytes) in {:?}",
        t.elapsed()
    );

    // Content-dedup census over every alias head: how many blob references
    // the live manifests make, how many distinct blobs that is, and what the
    // references would weigh if each one were its own file.
    let mut refs = 0u64;
    let mut declared = 0u64;
    let mut distinct: BTreeMap<[u8; 32], u64> = BTreeMap::new();
    let mut walked_aliases = 0u64;
    let mut cursor = None;
    loop {
        let page = core.search().search(&query, &viewer, cursor.as_deref()).expect("browse");
        for hit in &page.hits {
            let Some(alias) = hit.alias.as_deref() else { continue };
            let Ok(alias) = AssetAlias::new(alias) else { continue };
            let Some(target) = core.catalog().resolve_asset_alias(&alias).expect("resolve") else {
                continue;
            };
            walked_aliases += 1;
            let Some(bytes) = core
                .catalog()
                .asset_revision_manifest(&target.revision)
                .expect("manifest read")
            else {
                continue;
            };
            let manifest = AssetManifest::from_canonical_bytes(&bytes).expect("manifest");
            let mut note = |blob: &makepad_asset_data::BlobId, len: u64| {
                refs += 1;
                declared += len;
                distinct.insert(*blob.as_bytes(), len);
            };
            for f in &manifest.files {
                note(&f.blob, f.byte_len);
            }
            if let Some(t) = &manifest.thumbnail {
                note(&t.blob, t.byte_len);
            }
        }
        cursor = page.cursor;
        if cursor.is_none() {
            break;
        }
    }
    let unique_bytes: u64 = distinct.values().sum();
    println!(
        "dedup census over {walked_aliases} alias heads: {refs} blob references, \
         {} distinct blobs, declared {declared} bytes, distinct {unique_bytes} bytes, \
         saved {} bytes ({:.1}%)",
        distinct.len(),
        declared - unique_bytes,
        100.0 * (declared - unique_bytes) as f64 / declared.max(1) as f64,
    );
    println!("OK");
}

/// Directory fan-out of the CAS: depth, entries per level, biggest leaf.
fn cas_layout(objects: &Path) {
    let mut per_depth: BTreeMap<usize, u64> = BTreeMap::new();
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut max_leaf = (0u64, PathBuf::new());
    let mut stack = vec![(objects.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let mut here = 0u64;
        for entry in std::fs::read_dir(&dir).expect("read cas dir") {
            let entry = entry.expect("cas entry");
            let path = entry.path();
            if path.is_dir() {
                *per_depth.entry(depth + 1).or_default() += 1;
                stack.push((path, depth + 1));
            } else {
                files += 1;
                here += 1;
                bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        if here > max_leaf.0 {
            max_leaf = (here, dir);
        }
    }
    println!(
        "cas: {files} objects, {bytes} bytes, dirs per level {per_depth:?}, \
         busiest directory {} entries ({:?})",
        max_leaf.0, max_leaf.1
    );
}
