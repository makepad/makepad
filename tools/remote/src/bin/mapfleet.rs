//! mapfleet — distribute the world-cell bakes over makepad-remote boxes.
//!
//! This machine keeps the spool store and does the SLICING (I/O bound);
//! workers receive the sliced cell archive, run the BAKE (CPU bound) via
//! `cargo run -p makepad-map-bake`, and the dispatcher pulls the baked
//! cell home, weaving the world shard set after each arrival.
//!
//! Boxes run: `cargo run -p makepad-remote --release -- --server` from a
//! repo checkout (first job on each box compiles the bake tool, cached
//! after). Resume ledger = existing cell-NNN-baked.mbtiles files, shared
//! with world-slabs.sh — STOP that driver before running mapfleet.
//!
//!   mapfleet [--hosts ip:port,...] [--hosts-file fleet-hosts.txt] \
//!            [--cells tools/map_tiles/world-cells.txt] \
//!            [--store local/maps/world-detail.store] \
//!            [--out local/maps/world-cells] \
//!            [--mkmap local/maps/world.mkmap] [--local-worker]
//!
//! The hosts FILE is polled every 30s: append "ip:port" lines any time
//! and new compute nodes join the running spiral live.

#[path = "../protocol.rs"]
mod protocol;
use protocol::*;

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const BAKE_ARGS: &[&str] = &[
    "--zooms",
    "10,11,12,13,14",
    "--buckets",
    "15,16,17,18",
    "--threshold-ms",
    "100",
];
/// NL bbox: cells intersecting it bake with the bridge-dz sidecar.
const NL: (f64, f64, f64, f64) = (3.2, 50.7, 7.3, 53.6);
const BRIDGE_DZ: &str = "local/maps/nl-bridge-dz.mbtiles";

struct Config {
    hosts: Vec<String>,
    hosts_file: Option<PathBuf>,
    cells: PathBuf,
    store: PathBuf,
    out: PathBuf,
    mkmap: PathBuf,
    local_worker: bool,
}

struct Shared {
    cells: Vec<String>, // "w,s,e,n" per line, spiral order
    next: AtomicUsize,
    weave_dirty: AtomicBool,
    stop: AtomicBool,
    /// At most two concurrent slices: the store is read-only so parallel
    /// slicing is safe, and with 8 workers a single-slice rule becomes
    /// the pipeline limiter; more than two thrashes the disk.
    slice_gate: Mutex<usize>,
}

fn main() {
    let config = match parse_args() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("mapfleet: {err}");
            std::process::exit(1);
        }
    };
    let cells: Vec<String> = std::fs::read_to_string(&config.cells)
        .unwrap_or_else(|err| {
            eprintln!("mapfleet: read {}: {err}", config.cells.display());
            std::process::exit(1);
        })
        .lines()
        .map(str::to_string)
        .filter(|line| !line.is_empty())
        .collect();
    std::fs::create_dir_all(&config.out).ok();

    let shared = Arc::new(Shared {
        cells,
        next: AtomicUsize::new(0),
        weave_dirty: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        slice_gate: Mutex::new(0),
    });
    let config = Arc::new(config);

    let mut handles = Vec::new();
    let mut known_hosts: std::collections::HashSet<String> = Default::default();
    for host in config.hosts.clone() {
        known_hosts.insert(host.clone());
        let shared = shared.clone();
        let config = config.clone();
        handles.push(thread::spawn(move || remote_worker(&host, &shared, &config)));
    }
    if let Some(hosts_file) = config.hosts_file.clone() {
        let shared_watch = shared.clone();
        let config_watch = config.clone();
        handles.push(thread::spawn(move || loop {
            if shared_watch.next.load(Ordering::SeqCst) >= shared_watch.cells.len() {
                return;
            }
            if let Ok(text) = std::fs::read_to_string(&hosts_file) {
                for line in text.lines().map(str::trim) {
                    if line.is_empty() || line.starts_with('#') || known_hosts.contains(line) {
                        continue;
                    }
                    known_hosts.insert(line.to_string());
                    println!("mapfleet: compute node joined: {line}");
                    let host = line.to_string();
                    let shared = shared_watch.clone();
                    let config = config_watch.clone();
                    thread::spawn(move || remote_worker(&host, &shared, &config));
                }
            }
            thread::sleep(Duration::from_secs(30));
        }));
    }
    if config.local_worker || config.hosts.is_empty() {
        let shared = shared.clone();
        let config = config.clone();
        handles.push(thread::spawn(move || local_worker(&shared, &config)));
    }
    {
        let shared = shared.clone();
        let config = config.clone();
        handles.push(thread::spawn(move || weave_loop(&shared, &config)));
    }
    for handle in handles {
        let _ = handle.join();
    }
    println!("mapfleet: spiral complete");
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config {
        hosts: Vec::new(),
        hosts_file: None,
        cells: "tools/map_tiles/world-cells.txt".into(),
        store: "local/maps/world-detail.store".into(),
        out: "local/maps/world-cells".into(),
        mkmap: "local/maps/world.mkmap".into(),
        local_worker: false,
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        let take = |index: usize| -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{} needs a value", args[index]))
        };
        match args[index].as_str() {
            "--hosts" => {
                config.hosts = take(index)?
                    .split(',')
                    .map(str::to_string)
                    .filter(|h| !h.is_empty())
                    .collect();
                index += 2;
            }
            "--hosts-file" => {
                config.hosts_file = Some(take(index)?.into());
                index += 2;
            }
            "--cells" => {
                config.cells = take(index)?.into();
                index += 2;
            }
            "--store" => {
                config.store = take(index)?.into();
                index += 2;
            }
            "--out" => {
                config.out = take(index)?.into();
                index += 2;
            }
            "--mkmap" => {
                config.mkmap = take(index)?.into();
                index += 2;
            }
            "--local-worker" => {
                config.local_worker = true;
                index += 1;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(config)
}

fn wait_for_spool(config: &Config) {
    let marker = config.store.join("SPOOL_COMPLETE");
    while !marker.exists() {
        thread::sleep(Duration::from_secs(60));
    }
}

fn cell_paths(config: &Config, index: usize) -> (String, PathBuf, PathBuf) {
    let name = format!("cell-{:03}", index + 1);
    let base = config.out.join(format!("{name}-base.mbtiles"));
    let baked = config.out.join(format!("{name}-baked.mbtiles"));
    (name, base, baked)
}

fn claim_cell(shared: &Shared, config: &Config) -> Option<(usize, String)> {
    loop {
        let index = shared.next.fetch_add(1, Ordering::SeqCst);
        if index >= shared.cells.len() {
            return None;
        }
        let (_, _, baked) = cell_paths(config, index);
        if baked.exists() {
            continue; // resume ledger: already done
        }
        return Some((index, shared.cells[index].clone()));
    }
}

fn intersects_nl(bbox: &str) -> bool {
    let parts: Vec<f64> = bbox
        .split(',')
        .filter_map(|value| value.parse().ok())
        .collect();
    if parts.len() != 4 {
        return false;
    }
    parts[0] < NL.2 && parts[2] > NL.0 && parts[1] < NL.3 && parts[3] > NL.1
}

/// Slice a cell from the local store (serialized; minutes per cell).
/// Ok(false) = cell is empty ocean — mark done with an empty ledger file.
fn slice_cell(shared: &Shared, config: &Config, index: usize, bbox: &str) -> io::Result<bool> {
    let (name, base, _) = cell_paths(config, index);
    if base.exists() {
        return Ok(true);
    }
    loop {
        let mut active = shared.slice_gate.lock().unwrap();
        if *active < 2 {
            *active += 1;
            break;
        }
        drop(active);
        thread::sleep(Duration::from_secs(5));
    }
    println!("mapfleet: {name} slice {bbox}");
    let status = Command::new("./target/release/mptiles-run")
        .args(["pbf-base", "local/maps/pbf/planet-latest.osm.pbf"])
        .arg(&base)
        .args(["--store"])
        .arg(&config.store)
        .args(["--bbox", bbox])
        .status()?;
    *shared.slice_gate.lock().unwrap() -= 1;
    if !status.success() {
        let _ = std::fs::remove_file(&base);
        return Ok(false);
    }
    Ok(true)
}

fn mark_empty(config: &Config, index: usize) {
    let (name, _, baked) = cell_paths(config, index);
    println!("mapfleet: {name} empty — ledger placeholder");
    let _ = std::fs::write(&baked, b"");
}

fn local_worker(shared: &Shared, config: &Config) {
    wait_for_spool(config);
    while !shared.stop.load(Ordering::SeqCst) {
        let Some((index, bbox)) = claim_cell(shared, config) else {
            return;
        };
        let (name, base, baked) = cell_paths(config, index);
        match slice_cell(shared, config, index, &bbox) {
            Ok(false) => {
                mark_empty(config, index);
                continue;
            }
            Err(err) => {
                eprintln!("mapfleet: {name} slice error: {err}");
                continue;
            }
            Ok(true) => {}
        }
        println!("mapfleet: {name} bake (local)");
        let mut cmd = Command::new("./target/release/mpbake-run");
        cmd.arg(&base).arg(&baked).args(BAKE_ARGS);
        if intersects_nl(&bbox) {
            cmd.args(["--bridge-dz", BRIDGE_DZ]);
        }
        match cmd.status() {
            Ok(status) if status.success() => {
                let _ = std::fs::remove_file(&base);
                shared.weave_dirty.store(true, Ordering::SeqCst);
                println!("mapfleet: {name} baked (local)");
            }
            other => {
                let _ = std::fs::remove_file(&baked);
                eprintln!("mapfleet: {name} local bake failed: {other:?}");
            }
        }
    }
}

fn remote_worker(host: &str, shared: &Shared, config: &Config) {
    // Bootstrap: build the bake tool on the box (no-op when cached).
    println!("mapfleet: {host} bootstrap build");
    match remote_run(host, &["build", "-p", "makepad-map-bake", "--release"], &[]) {
        Ok(0) => println!("mapfleet: {host} bootstrap ready"),
        other => {
            eprintln!("mapfleet: {host} bootstrap failed ({other:?}) — worker disabled");
            return;
        }
    }
    // Bootstrap runs pre-marker (warm compile during the spool wait);
    // cell work gates on the honest marker like everyone else.
    wait_for_spool(config);
    let mut pushed_bridge_dz = false;
    while !shared.stop.load(Ordering::SeqCst) {
        let Some((index, bbox)) = claim_cell(shared, config) else {
            return;
        };
        let (name, base, baked) = cell_paths(config, index);
        match slice_cell(shared, config, index, &bbox) {
            Ok(false) => {
                mark_empty(config, index);
                continue;
            }
            Err(err) => {
                eprintln!("mapfleet: {name} slice error: {err}");
                continue;
            }
            Ok(true) => {}
        }
        let needs_dz = intersects_nl(&bbox);
        let mut files: Vec<(String, PathBuf)> =
            vec![(format!("fleet/{name}.mbtiles"), base.clone())];
        if needs_dz && !pushed_bridge_dz {
            files.push(("fleet/nl-bridge-dz.mbtiles".to_string(), BRIDGE_DZ.into()));
        }
        let remote_in = format!("fleet/{name}.mbtiles");
        let remote_out = format!("fleet/{name}-baked.mbtiles");
        let mut run: Vec<String> = vec![
            "run".into(),
            "-p".into(),
            "makepad-map-bake".into(),
            "--release".into(),
            "--".into(),
            remote_in.clone(),
            remote_out.clone(),
        ];
        run.extend(BAKE_ARGS.iter().map(|s| s.to_string()));
        if needs_dz {
            run.extend(["--bridge-dz".into(), "fleet/nl-bridge-dz.mbtiles".into()]);
        }
        println!("mapfleet: {name} bake on {host}");
        let run_refs: Vec<&str> = run.iter().map(String::as_str).collect();
        match remote_run(host, &run_refs, &files) {
            Ok(0) => {
                if needs_dz {
                    pushed_bridge_dz = true;
                }
                match remote_pull(host, &remote_out, &baked) {
                    Ok(()) => {
                        let _ = std::fs::remove_file(&base);
                        shared.weave_dirty.store(true, Ordering::SeqCst);
                        println!("mapfleet: {name} baked on {host}");
                    }
                    Err(err) => {
                        eprintln!("mapfleet: {name} pull from {host} failed: {err} — requeue");
                        let _ = std::fs::remove_file(&baked);
                        requeue(shared, index);
                    }
                }
            }
            other => {
                eprintln!("mapfleet: {name} on {host} failed ({other:?}) — requeue, backoff");
                requeue(shared, index);
                thread::sleep(Duration::from_secs(30));
            }
        }
    }
}

/// A failed cell must not be lost: rewind the claim cursor so any worker
/// (including this one after backoff) can pick it up. The ledger check in
/// claim_cell makes double-processing harmless.
fn requeue(shared: &Shared, index: usize) {
    let _ = shared
        .next
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            (current > index).then_some(index)
        });
}

fn remote_connect(host: &str) -> io::Result<TcpStream> {
    let stream = TcpStream::connect(host)?;
    stream.set_nodelay(true).ok();
    Ok(stream)
}

fn remote_run(host: &str, cargo_args: &[&str], files: &[(String, PathBuf)]) -> io::Result<i32> {
    let mut stream = remote_connect(host)?;
    for (remote_path, local_path) in files {
        let data = std::fs::read(local_path)?;
        println!(
            "mapfleet: {host} <- {} ({:.1} MB)",
            remote_path,
            data.len() as f64 / 1e6
        );
        let payload = encode_file_data(remote_path, &data);
        write_msg(&mut stream, TAG_FILE_DATA, &payload)?;
    }
    write_msg(&mut stream, TAG_CARGO_RUN, cargo_args.join("\n").as_bytes())?;
    let mut exit_code = 1;
    loop {
        let (tag, payload) = read_msg(&mut stream)?;
        match tag {
            TAG_OUTPUT => { /* worker logs stay on the worker */ }
            TAG_EXIT_CODE => {
                if payload.len() >= 4 {
                    exit_code =
                        i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                }
                break;
            }
            TAG_ERROR => {
                eprintln!(
                    "mapfleet: {host} error: {}",
                    String::from_utf8_lossy(&payload)
                );
                break;
            }
            _ => break,
        }
    }
    stream.shutdown(std::net::Shutdown::Both).ok();
    Ok(exit_code)
}

fn remote_pull(host: &str, remote_path: &str, local_path: &Path) -> io::Result<()> {
    let mut stream = remote_connect(host)?;
    write_msg(&mut stream, TAG_FILE_PULL, remote_path.as_bytes())?;
    let (tag, payload) = read_msg(&mut stream)?;
    if tag != TAG_FILE_DATA {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("pull answered tag 0x{tag:02x}"),
        ));
    }
    let (_, data) = decode_file_data(&payload)?;
    println!(
        "mapfleet: {host} -> {} ({:.1} MB)",
        local_path.display(),
        data.len() as f64 / 1e6
    );
    std::fs::write(local_path, data)?;
    Ok(())
}

/// Re-weave the world shard set whenever new cells land; coalesces bursts.
fn weave_loop(shared: &Shared, config: &Config) {
    loop {
        thread::sleep(Duration::from_secs(10));
        let done = shared.next.load(Ordering::SeqCst) >= shared.cells.len();
        if !shared.weave_dirty.swap(false, Ordering::SeqCst) {
            if done {
                return;
            }
            continue;
        }
        let mut baked: Vec<PathBuf> = Vec::new();
        for index in 0..shared.cells.len() {
            let (_, _, path) = cell_paths(config, index);
            // Empty ledger placeholders mark ocean cells; skip them.
            if path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                baked.push(path);
            }
        }
        if baked.is_empty() {
            continue;
        }
        println!("mapfleet: weaving {} cells", baked.len());
        let next_dir = config.mkmap.with_extension("mkmap.next");
        let prev_dir = config.mkmap.with_extension("mkmap.prev");
        let _ = std::fs::remove_dir_all(&next_dir);
        let mut cmd = Command::new("./target/release/mptiles-run");
        cmd.arg("transmux");
        for path in &baked {
            cmd.arg(path);
        }
        cmd.arg(&next_dir);
        match cmd.status() {
            Ok(status) if status.success() => {
                let _ = std::fs::remove_dir_all(&prev_dir);
                if config.mkmap.exists() {
                    let _ = std::fs::rename(&config.mkmap, &prev_dir);
                }
                match std::fs::rename(&next_dir, &config.mkmap) {
                    Ok(()) => println!("mapfleet: world LIVE with {} cells", baked.len()),
                    Err(err) => eprintln!("mapfleet: swap failed: {err}"),
                }
            }
            other => eprintln!("mapfleet: weave failed: {other:?}"),
        }
    }
}
