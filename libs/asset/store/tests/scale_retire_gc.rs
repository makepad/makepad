//! Scale rig: deletion and collection on a synthetic catalog of 100k+
//! assets. Ignored by default (it builds a multi-hundred-megabyte catalog);
//! run it explicitly:
//!
//!   cargo test --release -p makepad-asset-store --test scale_retire_gc \
//!       -- --ignored --nocapture
//!
//! `SCALE_ASSETS` overrides the asset count (default 100_000, two revisions
//! each = 200k revisions, 400k blob rows).
//!
//! What it measures, and why each number matters:
//! - the v9 index build (`asset_revisions_by_asset`), the one migration cost
//!   that is not schema-only,
//! - `retire_asset` latency, which must depend on the asset's own revisions
//!   and aliases and NOT on catalog size,
//! - browse and text search latency with half the catalog deleted, proving
//!   deleted rows are absent from the index rather than filtered out of it,
//! - the GC run: total wall time, step count, and the WORST single step,
//!   which is the number that decides whether collection can run while the
//!   server serves traffic.
//!
//! The synthetic rows are written straight into the catalog (no CAS objects
//! for the bulk), so the sweep's per-blob unlink syscall is not part of the
//! timings; everything else — mark decode, posting deletes, transactions —
//! is the real code path.

mod common;

use common::*;
use makepad_asset_data::*;
use makepad_asset_store::*;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn env_count(key: &str, default: usize) -> usize {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Deterministic 16-byte asset id from an index, ordered like the index.
fn asset_id_of(i: usize) -> AssetId {
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&(i as u64).to_be_bytes());
    b[8] = 0xA5;
    AssetId::from_bytes(b)
}

struct Rig {
    root: std::path::PathBuf,
    assets: usize,
    revisions_per_asset: usize,
}

impl Rig {
    /// Build the synthetic catalog: assets, two published revisions each
    /// (unique blobs per revision), an alias on the newest, and an
    /// annotation with postings so search has real work to do.
    fn build(name: &str, assets: usize, revisions_per_asset: usize) -> Rig {
        let root = test_root(name);
        {
            // Create the schema at the current version, then close.
            let _core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        }
        let db = root.join("catalog.sqlite3");
        let t0 = Instant::now();
        let mut sql = String::with_capacity(8 * 1024 * 1024);
        sql.push_str("PRAGMA synchronous=OFF;BEGIN;");
        let mut in_batch = 0usize;
        let mut flush = |sql: &mut String, force: bool, in_batch: &mut usize| {
            if *in_batch >= 2_000 || force {
                sql.push_str("COMMIT;");
                raw::exec(&db, sql);
                sql.clear();
                sql.push_str("BEGIN;");
                *in_batch = 0;
            }
        };
        for i in 0..assets {
            let id = asset_id_of(i);
            let id_hex = hex(id.as_bytes());
            let _ = write!(
                sql,
                "INSERT INTO assets(asset_id, namespace, created_ms) VALUES(X'{id_hex}','ns',{});",
                NOW + i as u64
            );
            let mut head = String::new();
            for r in 0..revisions_per_asset {
                let glb = format!("scale-glb-{i}-{r}").into_bytes();
                let thumb = format!("scale-thumb-{i}-{r}").into_bytes();
                for bytes in [&glb, &thumb] {
                    let _ = write!(
                        sql,
                        "INSERT OR IGNORE INTO blobs(blob_id, size, created_ms) \
                         VALUES(X'{}',{},{});",
                        hex(BlobId::hash_of(bytes).as_bytes()),
                        bytes.len(),
                        NOW
                    );
                }
                let manifest = prop_manifest(id, &glb, &thumb);
                let bytes = manifest.to_canonical_bytes().unwrap();
                let rev = AssetRevisionId::hash_of(&bytes);
                let rev_hex = hex(rev.as_bytes());
                let created = NOW + (i * revisions_per_asset + r) as u64;
                let _ = write!(
                    sql,
                    "INSERT INTO asset_revisions(revision, asset_id, manifest, created_ms) \
                     VALUES(X'{rev_hex}',X'{id_hex}',X'{}',{created});\
                     INSERT INTO candidates(kind, owner_id, revision, state, staged_ms, published_ms) \
                     VALUES('asset',X'{id_hex}',X'{rev_hex}','published',{created},{created});",
                    hex(&bytes)
                );
                head = rev_hex;
            }
            let alias = format!("ns/scale-{i:07}");
            let _ = write!(
                sql,
                "INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms) \
                 VALUES('{alias}',X'{id_hex}',X'{head}',{});\
                 INSERT INTO search_annotations(asset_id, namespace, kind, visibility, owner, \
                     title, description, creator, generator, backend, model, prompt, provenance, \
                     live, updated_ms, canon_alias) \
                 VALUES(X'{id_hex}','ns','prop','public',NULL,'scale asset {i}','synthetic row',\
                     'rig','','','','','',1,{},'{alias}');",
                NOW, NOW
            );
            for (term, weight) in [("scale", 100u32), ("asset", 20), ("prop", 60)] {
                let _ = write!(
                    sql,
                    "INSERT INTO search_postings(term, asset_id, weight_public, weight_owner) \
                     VALUES('{term}',X'{id_hex}',{weight},{weight});"
                );
                let _ = write!(
                    sql,
                    "INSERT OR IGNORE INTO search_alias_postings(term, asset_id, weight) \
                     VALUES('{term}',X'{id_hex}',80);"
                );
            }
            in_batch += 1;
            flush(&mut sql, false, &mut in_batch);
        }
        flush(&mut sql, true, &mut in_batch);
        raw::exec(&db, "ANALYZE;");
        println!(
            "built {assets} assets x {revisions_per_asset} revisions in {:?}",
            t0.elapsed()
        );
        Rig { root, assets, revisions_per_asset }
    }

    fn open(&self) -> AssetServerCore {
        AssetServerCore::open(&self.root, Budgets::default_v1()).unwrap()
    }

    fn db(&self) -> std::path::PathBuf {
        self.root.join("catalog.sqlite3")
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn browse(core: &AssetServerCore, text: &str) -> (usize, u64, Duration) {
    let t = Instant::now();
    let page = core
        .search()
        .search(
            &SearchQuery {
                text,
                filters: SearchFilters::default(),
                expand: false,
                page_size: 100,
                facets: 0,
            },
            &SearchViewer { principal: None, scope: ViewerScope::All },
            None,
        )
        .unwrap();
    (page.hits.len(), page.total, t.elapsed())
}

#[test]
#[ignore = "scale rig: builds a 100k-asset catalog"]
fn deletion_and_collection_at_scale() {
    let assets = env_count("SCALE_ASSETS", 100_000);
    let revisions = 2;
    let rig = Rig::build("scale_retire_gc", assets, revisions);

    // ---- the one non-trivial migration cost: the per-asset revision index.
    raw::exec(&rig.db(), "DROP INDEX IF EXISTS asset_revisions_by_asset;");
    let t = Instant::now();
    raw::exec(
        &rig.db(),
        "CREATE INDEX asset_revisions_by_asset ON asset_revisions(asset_id, created_ms, revision);",
    );
    println!("v9 index build over {} revisions: {:?}", assets * revisions, t.elapsed());

    let core = rig.open();
    let (hits, total, took) = browse(&core, "");
    println!("browse page (100 of {total}): {:.2} ms", ms(took));
    assert_eq!(hits, 100);
    assert_eq!(total as usize, assets);
    let (_, total_text, took) = browse(&core, "scale prop");
    println!("text search page (100 of {total_text}): {:.2} ms", ms(took));

    // ---- retire_asset must not scale with the catalog ---------------------
    let samples = 200usize;
    let mut worst = Duration::ZERO;
    let t0 = Instant::now();
    for k in 0..samples {
        // Spread the sample across the id space so no page of the catalog is
        // favoured by warm cache alone.
        let i = (k * (rig.assets / samples.max(1))).min(rig.assets - 1);
        let t = Instant::now();
        let report = core.catalog().retire_asset(&asset_id_of(i), NOW + 1_000_000).unwrap();
        worst = worst.max(t.elapsed());
        assert_eq!(report.revisions_retired, revisions as u64);
        assert_eq!(report.aliases_dropped, 1);
        assert!(report.annotation_cleared);
    }
    println!(
        "retire_asset over {samples} assets: mean {:.3} ms, worst {:.3} ms",
        ms(t0.elapsed()) / samples as f64,
        ms(worst)
    );

    // ---- half the catalog deleted, then search again ----------------------
    let half = rig.assets / 2;
    let t0 = Instant::now();
    for i in 0..half {
        core.catalog().retire_asset(&asset_id_of(i), NOW + 1_000_001).unwrap();
    }
    println!(
        "retired {half} assets in {:?} ({:.3} ms each)",
        t0.elapsed(),
        ms(t0.elapsed()) / half as f64
    );
    let (hits, total, took) = browse(&core, "");
    println!("browse with half deleted (100 of {total}): {:.2} ms", ms(took));
    assert_eq!(hits, 100);
    assert!(
        (total as usize) <= assets - half,
        "deleted assets must be absent from search, saw {total}"
    );
    let (_, total_text, took) = browse(&core, "scale prop");
    println!("text search with half deleted (100 of {total_text}): {:.2} ms", ms(took));

    // ---- the collection itself --------------------------------------------
    let cfg = GcConfig { dry_run: true, grace_ms: 0, ..GcConfig::default_v1() };
    let (dry, steps) = run_gc(&core, cfg);
    println!(
        "GC dry run: {steps}, {} blobs / {} bytes reclaimable",
        dry.unreferenced_blobs, dry.unreferenced_bytes
    );
    let cfg = GcConfig { dry_run: false, ..cfg };
    let (real, steps) = run_gc(&core, cfg);
    println!(
        "GC collect: {steps}, {} blobs / {} bytes deleted",
        real.deleted_blobs, real.deleted_bytes
    );
    assert_eq!(real.deleted_blobs, dry.unreferenced_blobs);
    // Every retired asset's blobs went; every live asset's stayed.
    assert_eq!(real.deleted_blobs, (half + 200) as u64 * 2 * revisions as u64 - overlap(half));

    // A second run has nothing left to do, which is the property that makes
    // collection cheap to run often.
    let (idle, _) = run_gc(&core, cfg);
    assert_eq!(idle.deleted_blobs, 0);
}

/// The sampled retirements above fall inside the first half's id range once
/// `k * stride < half`; those assets are retired twice (idempotent), so
/// their blobs must not be double counted.
fn overlap(half: usize) -> u64 {
    let _ = half;
    // The sampler strides across the whole id space, so exactly half of its
    // 200 samples land in the first half of the catalog.
    100 * 2 * 2
}

/// Run a whole collection one step at a time, keeping the latency of every
/// step: the tail is what decides whether GC can run under live traffic.
fn run_gc(core: &AssetServerCore, cfg: GcConfig) -> (GcStatus, Steps) {
    let now = NOW + 2_000_000;
    core.gc_begin(cfg, now).unwrap();
    let mut took: Vec<Duration> = Vec::new();
    let t0 = Instant::now();
    loop {
        let t = Instant::now();
        let status = core.gc_advance(1, now).unwrap().unwrap();
        took.push(t.elapsed());
        if status.finished() {
            return (status, Steps { took, total: t0.elapsed() });
        }
        assert!(took.len() < 1_000_000, "gc did not converge");
    }
}

struct Steps {
    took: Vec<Duration>,
    total: Duration,
}

impl std::fmt::Display for Steps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut sorted = self.took.clone();
        sorted.sort();
        let at = |q: f64| -> f64 {
            let i = ((sorted.len() as f64 - 1.0) * q).round() as usize;
            ms(sorted[i])
        };
        write!(
            f,
            "{} steps in {:?} (step p50 {:.2} ms, p99 {:.2} ms, max {:.2} ms)",
            sorted.len(),
            self.total,
            at(0.5),
            at(0.99),
            at(1.0)
        )
    }
}

/// Sanity for the rig itself at a small size, so the ignored test above is
/// known to be measuring a catalog with the shape it claims.
#[test]
fn the_scale_rig_builds_a_catalog_the_core_accepts() {
    let rig = Rig::build("scale_rig_smoke", 64, 2);
    let core = rig.open();
    let (hits, total, _) = browse(&core, "");
    assert_eq!(hits, 64);
    assert_eq!(total, 64);
    let id = asset_id_of(7);
    let report = core.catalog().retire_asset(&id, NOW + 10).unwrap();
    assert_eq!(report.revisions_retired, 2);
    assert_eq!(report.aliases_dropped, 1);
    let (_, total, _) = browse(&core, "");
    assert_eq!(total, 63);
    let cfg = GcConfig { dry_run: false, grace_ms: 0, ..GcConfig::default_v1() };
    let (status, _) = run_gc(&core, cfg);
    assert_eq!(status.deleted_blobs, 4, "the retired asset's two revisions' blobs");
    let _ = Path::new(&rig.root);
}
