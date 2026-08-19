//! Measure (and optionally perform) blob garbage collection on a server
//! root: how many blobs and bytes are unreachable, how long the incremental
//! run takes, and what one step costs.
//!
//! Opening a root migrates its schema, and `--collect` DELETES bytes. Point
//! it at a COPY of a live root, never at one a server is using.
//!
//!   cargo run --release -p makepad-asset-store --example gc_probe -- <root>
//!   cargo run --release -p makepad-asset-store --example gc_probe -- <root> --retain 1
//!   cargo run --release -p makepad-asset-store --example gc_probe -- <root> --collect
//!
//! Default is a DRY RUN: nothing is deleted and nothing is retired.
//! `--retain N` previews (or applies) the retention rule that keeps the
//! newest N revisions per asset plus every alias head.

use makepad_asset_store::{AssetServerCore, Budgets, GcConfig, GcStatus};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = match args.next() {
        Some(r) => PathBuf::from(r),
        None => {
            eprintln!("usage: gc_probe <server-root> [--collect] [--retain N] [--grace-ms N]");
            std::process::exit(2);
        }
    };
    let mut cfg = GcConfig { dry_run: true, grace_ms: 0, ..GcConfig::default_v1() };
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--collect" => cfg.dry_run = false,
            "--retain" => {
                let n: u32 = args.next().and_then(|v| v.parse().ok()).expect("--retain N");
                cfg.retain_keep = Some(n);
            }
            "--grace-ms" => {
                cfg.grace_ms = args.next().and_then(|v| v.parse().ok()).expect("--grace-ms N");
            }
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    let t0 = Instant::now();
    let core = AssetServerCore::open(&root, Budgets::default_v1()).expect("open root");
    println!("open + migrate: {:?}", t0.elapsed());
    let recovered = core.recover(now_ms()).expect("recover");
    println!(
        "recover: {} cas temps, {} pending deletes, {} leases",
        recovered.cas_temps_removed, recovered.gc_deletes_resolved, recovered.leases_expired
    );

    let now = now_ms();
    println!(
        "mode: {} retain={:?} grace_ms={}",
        if cfg.dry_run { "DRY RUN" } else { "COLLECT" },
        cfg.retain_keep,
        cfg.grace_ms
    );
    core.gc_begin(cfg, now).expect("gc begin");
    let t0 = Instant::now();
    let mut steps = 0u64;
    let mut worst = Duration::ZERO;
    let status: GcStatus = loop {
        let t = Instant::now();
        let status = core.gc_advance(1, now).expect("gc step").expect("gc run");
        let took = t.elapsed();
        steps += 1;
        worst = worst.max(took);
        if steps % 200 == 0 {
            println!(
                "  .. step {steps} phase={} scanned={} marked={} examined={} freed={} bytes",
                status.phase.as_str(),
                status.scanned_revisions,
                status.marked_blobs,
                status.examined_blobs,
                status.unreferenced_bytes
            );
        }
        if status.finished() {
            break status;
        }
    };
    println!("run {} in {:?} over {steps} steps (worst step {worst:?})", status.run_id, t0.elapsed(), );
    println!("  phase              {}", status.phase.as_str());
    println!("  retired revisions  {}", status.retired_revisions);
    println!("  documents scanned  {}", status.scanned_revisions);
    println!("  blobs referenced   {}", status.marked_blobs);
    println!("  blobs examined     {}", status.examined_blobs);
    println!(
        "  unreferenced       {} blobs, {} bytes ({:.1} MiB)",
        status.unreferenced_blobs,
        status.unreferenced_bytes,
        status.unreferenced_bytes as f64 / (1024.0 * 1024.0)
    );
    println!(
        "  deleted            {} blobs, {} bytes ({:.1} MiB)",
        status.deleted_blobs,
        status.deleted_bytes,
        status.deleted_bytes as f64 / (1024.0 * 1024.0)
    );
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
}
