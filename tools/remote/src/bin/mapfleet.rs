//! mapfleet — distribute the world-cell bakes over makepad-remote boxes.
//!
//! This machine keeps the spool store and does the SLICING (I/O bound);
//! workers receive the sliced cell archive, run the BAKE (CPU bound) via
//! `cargo run -p makepad-map-bake`, and the dispatcher pulls the baked
//! cell home, weaving the world shard set after each arrival.
//!
//! Boxes run: `cargo run -p makepad-remote --release -- --server` from a
//! repo checkout (first job on each box compiles the bake tool, cached
//! after). Resume ledger = existing cell-NNN-baked.mbtiles files.
//!
//! Streaming: cell work does NOT wait for the whole spool. Pass 4
//! publishes store/spool-frontier.txt (spiral distance below which every
//! relation is on disk); a cell may slice once the frontier strictly
//! clears its farthest corner, so the fleet starts on NL while the spool
//! is still assembling the far side of the planet.
//!
//!   mapfleet [--hosts ip:port,...] [--hosts-file fleet-hosts.txt] \
//!            [--cells tools/map_tiles/world-cells.txt] \
//!            [--store local/maps/world-detail.store] \
//!            [--pbf local/maps/pbf/planet-latest.osm.pbf] \
//!            [--out local/maps/world-cells] \
//!            [--mkmap local/maps/world.mkmap] [--local-worker]
//!
//! The hosts FILE is polled every 30s: append "ip:port" lines any time
//! and new compute nodes join the running spiral live.

#[path = "../protocol.rs"]
#[allow(dead_code)] // shared with main.rs; each binary uses a subset
mod protocol;
use protocol::*;

use std::io;
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
    // The whole fleet does the compression: every tile re-encodes at q11
    // on the worker; the slice ships a throwaway-q2 intermediate.
    "--brotli-quality",
    "11",
    "--recompress",
];
/// NL bbox: cells intersecting it bake with the bridge-dz sidecar.
const NL: (f64, f64, f64, f64) = (3.2, 50.7, 7.3, 53.6);
const BRIDGE_DZ: &str = "local/maps/nl-bridge-dz.mbtiles";

struct Config {
    hosts: Vec<String>,
    hosts_file: Option<PathBuf>,
    cells: PathBuf,
    store: PathBuf,
    pbf: PathBuf,
    out: PathBuf,
    mkmap: PathBuf,
    local_worker: bool,
}

struct Shared {
    cells: Vec<String>, // "w,s,e,n" per line, spiral order
    next: AtomicUsize,
    /// Cells a worker is actively slicing/baking. The final weave waits
    /// for this to drain so late arrivals are never left out of the shard.
    active: AtomicUsize,
    weave_dirty: AtomicBool,
    stop: AtomicBool,
    /// Cells whose slice is too large to ship over the wire — only the
    /// local worker may take them.
    local_only: Mutex<std::collections::HashSet<usize>>,
    /// At most three concurrent slices: the store is read-only so parallel
    /// slicing is safe; ten workers need a fresh cell every couple of
    /// minutes and the NVMe sustains three readers without thrash.
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
    sweep_stale_sort_chunks(&config);

    let shared = Arc::new(Shared {
        cells,
        next: AtomicUsize::new(0),
        active: AtomicUsize::new(0),
        weave_dirty: AtomicBool::new(false),
        stop: AtomicBool::new(false),
        local_only: Mutex::new(Default::default()),
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
    let weave_handle = {
        let shared = shared.clone();
        let config = config.clone();
        thread::spawn(move || weave_loop(&shared, &config))
    };
    for handle in handles {
        let _ = handle.join();
    }
    // Hot-joined workers (hosts-file watcher) are detached — drain them.
    while shared.next.load(Ordering::SeqCst) < shared.cells.len()
        || shared.active.load(Ordering::SeqCst) > 0
    {
        thread::sleep(Duration::from_secs(10));
    }
    shared.stop.store(true, Ordering::SeqCst);
    let _ = weave_handle.join();
    // One last weave so the final arrivals are in the served shard set.
    weave(&shared, &config);
    println!("mapfleet: spiral complete");
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config {
        hosts: Vec::new(),
        hosts_file: None,
        cells: "tools/map_tiles/world-cells.txt".into(),
        store: "local/maps/world-detail.store".into(),
        pbf: "local/maps/pbf/planet-latest.osm.pbf".into(),
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
            "--pbf" => {
                config.pbf = take(index)?.into();
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

/// Slicers (pbf-base) suffix sort chunks with their pid; a crashed slice
/// leaks its chunks. Swept at dispatcher startup, when no slicer runs.
fn sweep_stale_sort_chunks(config: &Config) {
    let spool = config.store.join("spool");
    let Ok(entries) = std::fs::read_dir(&spool) else {
        return;
    };
    let mut swept = 0_u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("block-") && name.contains(".sort-") {
            if std::fs::remove_file(entry.path()).is_ok() {
                swept += 1;
            }
        }
    }
    if swept > 0 {
        println!("mapfleet: swept {swept} stale sort chunks");
    }
}

/// Same web-mercator global units as tools/map_tiles native/geom.rs
/// project_lon_lat — the frontier file publishes distances in this metric.
const DETAIL_ZOOM: u8 = 14;
const MVT_EXTENT: f64 = 4096.0;
const SPIRAL_ANCHOR: (f64, f64) = (5.2, 52.2);
/// Stricter than pbf-base's own 1-tile margin so its gate stays cold.
const FRONTIER_MARGIN: f64 = 2.0 * MVT_EXTENT;

fn project_lon_lat(lon: f64, lat: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.06, 85.06);
    let world = ((1_u64 << DETAIL_ZOOM) as f64) * MVT_EXTENT;
    let x = (lon + 180.0) / 360.0 * world;
    let sin_lat = lat.to_radians().sin();
    let y =
        (0.5 - ((1.0 + sin_lat) / (1.0 - sin_lat)).ln() / (4.0 * std::f64::consts::PI)) * world;
    (x, y)
}

/// A cell is ready when the spool is complete, or when the pass-4
/// streaming frontier strictly clears the distance from the NL anchor to
/// the cell's FARTHEST corner: no unfinished relation can touch it.
fn cell_ready(config: &Config, bbox: &str) -> bool {
    if config.store.join("SPOOL_COMPLETE").exists()
        || config.store.join("spool.complete.json").exists()
    {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(config.store.join("spool-frontier.txt")) else {
        return false;
    };
    let Ok(frontier) = text.trim().parse::<f64>() else {
        return false;
    };
    let parts: Vec<f64> = bbox
        .split(',')
        .filter_map(|value| value.parse().ok())
        .collect();
    if parts.len() != 4 {
        return false;
    }
    let (anchor_x, anchor_y) = project_lon_lat(SPIRAL_ANCHOR.0, SPIRAL_ANCHOR.1);
    let mut far = 0.0_f64;
    for lon in [parts[0], parts[2]] {
        for lat in [parts[1], parts[3]] {
            let (x, y) = project_lon_lat(lon, lat);
            far = far.max(((x - anchor_x).powi(2) + (y - anchor_y).powi(2)).sqrt());
        }
    }
    frontier > far + FRONTIER_MARGIN
}

fn cell_paths(config: &Config, index: usize) -> (String, PathBuf, PathBuf) {
    let name = format!("cell-{:03}", index + 1);
    let base = config.out.join(format!("{name}-base.mbtiles"));
    let baked = config.out.join(format!("{name}-baked.mbtiles"));
    (name, base, baked)
}

fn claim_cell(shared: &Shared, config: &Config, remote: bool) -> Option<(usize, String)> {
    loop {
        let index = shared.next.fetch_add(1, Ordering::SeqCst);
        if index >= shared.cells.len() {
            return None;
        }
        let (_, _, baked) = cell_paths(config, index);
        if baked.exists() {
            continue; // resume ledger: already done
        }
        if remote && shared.local_only.lock().unwrap().contains(&index) {
            continue;
        }
        // Cells are in NL-spiral order — the same order the pass-4
        // frontier advances — so each worker blocks on its own claimed
        // cell and they unblock in sequence as the frontier grows.
        let mut waited = false;
        while !cell_ready(config, &shared.cells[index]) {
            if shared.stop.load(Ordering::SeqCst) {
                return None;
            }
            if !waited {
                println!(
                    "mapfleet: cell-{:03} waiting for streaming frontier",
                    index + 1
                );
                waited = true;
            }
            thread::sleep(Duration::from_secs(30));
        }
        shared.active.fetch_add(1, Ordering::SeqCst);
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
        if *active < 3 {
            *active += 1;
            break;
        }
        drop(active);
        thread::sleep(Duration::from_secs(5));
    }
    println!("mapfleet: {name} slice {bbox}");
    // Slice into a temp name and rename on success: base.exists() is the
    // "slice complete" signal and must never match a half-written file.
    let base_tmp = base.with_extension("mbtiles.tmp");
    let _ = std::fs::remove_file(&base_tmp);
    let output = Command::new("./target/release/mptiles-run")
        .arg("pbf-base")
        .arg(&config.pbf)
        .arg(&base_tmp)
        .args(["--store"])
        .arg(&config.store)
        .args(["--bbox", bbox])
        // Throwaway intermediate: the fleet re-encodes everything at q11.
        .args(["--brotli-quality", "2"])
        .output()?;
    *shared.slice_gate.lock().unwrap() -= 1;
    if output.status.success() {
        std::fs::rename(&base_tmp, &base)?;
    } else {
        let _ = std::fs::remove_file(&base_tmp);
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // The only benign failure: a bbox with no spool blocks is ocean.
        if text.contains("selects no spool blocks") {
            return Ok(false);
        }
        return Err(io::Error::other(format!(
            "pbf-base failed: {}",
            text.lines().rev().take(4).collect::<Vec<_>>().join(" | ")
        )));
    }
    Ok(true)
}

fn mark_empty(config: &Config, index: usize) {
    let (name, _, baked) = cell_paths(config, index);
    println!("mapfleet: {name} empty — ledger placeholder");
    let _ = std::fs::write(&baked, b"");
}

fn local_worker(shared: &Shared, config: &Config) {
    while !shared.stop.load(Ordering::SeqCst) {
        let Some((index, bbox)) = claim_cell(shared, config, false) else {
            return;
        };
        let (name, base, baked) = cell_paths(config, index);
        match slice_cell(shared, config, index, &bbox) {
            Ok(false) => {
                mark_empty(config, index);
                shared.active.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            Err(err) => {
                eprintln!("mapfleet: {name} slice error: {err} — requeue");
                requeue(shared, index);
                shared.active.fetch_sub(1, Ordering::SeqCst);
                thread::sleep(Duration::from_secs(30));
                continue;
            }
            Ok(true) => {}
        }
        println!("mapfleet: {name} bake (local)");
        // Bake into a temp name: the ledger name must appear atomically —
        // the weaver and the resume ledger both trust its existence.
        let baked_tmp = baked.with_extension("mbtiles.tmp");
        let _ = std::fs::remove_file(&baked_tmp);
        let mut cmd = Command::new("./target/release/mpbake-run");
        cmd.arg(&base).arg(&baked_tmp).args(BAKE_ARGS);
        if intersects_nl(&bbox) {
            cmd.args(["--bridge-dz", BRIDGE_DZ]);
        }
        match cmd.status() {
            Ok(status) if status.success() && std::fs::rename(&baked_tmp, &baked).is_ok() => {
                let _ = std::fs::remove_file(&base);
                shared.weave_dirty.store(true, Ordering::SeqCst);
                println!("mapfleet: {name} baked (local)");
            }
            other => {
                let _ = std::fs::remove_file(&baked_tmp);
                eprintln!("mapfleet: {name} local bake failed: {other:?}");
            }
        }
        shared.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn remote_worker(host: &str, shared: &Shared, config: &Config) {
    // Bootstrap: build the bake tool on the box (no-op when cached).
    println!("mapfleet: {host} bootstrap build");
    loop {
        match remote_run(host, &["build", "-p", "makepad-map-bake", "--release"], &[]) {
            Ok(0) => {
                println!("mapfleet: {host} bootstrap ready");
                break;
            }
            other => {
                // Box may be mid-fix (toolchain, link.exe, git pull) —
                // keep retrying so it joins the moment it is healthy.
                eprintln!("mapfleet: {host} bootstrap failed ({other:?}) — retry in 5min");
                thread::sleep(Duration::from_secs(300));
            }
        }
    }
    // Bootstrap runs while the spool spins up; cell work self-gates on
    // the streaming frontier inside claim_cell.
    let mut pushed_bridge_dz = false;
    let mut last_fail: Option<usize> = None;
    while !shared.stop.load(Ordering::SeqCst) {
        let Some((index, bbox)) = claim_cell(shared, config, true) else {
            return;
        };
        let (name, base, baked) = cell_paths(config, index);
        match slice_cell(shared, config, index, &bbox) {
            Ok(false) => {
                mark_empty(config, index);
                shared.active.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            Err(err) => {
                eprintln!("mapfleet: {name} slice error: {err} — requeue");
                requeue(shared, index);
                shared.active.fetch_sub(1, Ordering::SeqCst);
                thread::sleep(Duration::from_secs(30));
                continue;
            }
            Ok(true) => {}
        }
        // Oversized slices cannot ship (2GB socket-write ceiling, worker
        // RAM); hand them to the local worker and move on.
        let slice_bytes = std::fs::metadata(&base).map(|m| m.len()).unwrap_or(0);
        if slice_bytes > 3_500_000_000 {
            println!(
                "mapfleet: {name} slice {:.1} GB too large for the wire — local-only",
                slice_bytes as f64 / 1e9
            );
            shared.local_only.lock().unwrap().insert(index);
            requeue(shared, index);
            shared.active.fetch_sub(1, Ordering::SeqCst);
            continue;
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
                        requeue(shared, index);
                    }
                }
            }
            other => {
                if last_fail == Some(index) {
                    // Second consecutive failure of this cell on this box
                    // (low RAM/disk for the frame, sick toolchain, ...):
                    // hand the cell to the local worker and move on.
                    eprintln!(
                        "mapfleet: {name} failed twice on {host} ({other:?}) — routing local"
                    );
                    shared.local_only.lock().unwrap().insert(index);
                    last_fail = None;
                } else {
                    eprintln!(
                        "mapfleet: {name} on {host} failed ({other:?}) — requeue, backoff"
                    );
                    last_fail = Some(index);
                }
                requeue(shared, index);
                shared.active.fetch_sub(1, Ordering::SeqCst);
                thread::sleep(Duration::from_secs(30));
                continue;
            }
        }
        last_fail = None;
        shared.active.fetch_sub(1, Ordering::SeqCst);
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
    // Temp + rename: the ledger name must never exist half-written.
    let tmp = local_path.with_extension("mbtiles.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, local_path)?;
    Ok(())
}

/// Re-weave the world shard set whenever new cells land; coalesces bursts.
/// Runs until the stop flag; main does one final weave() after the drain.
fn weave_loop(shared: &Shared, config: &Config) {
    loop {
        thread::sleep(Duration::from_secs(10));
        if shared.weave_dirty.swap(false, Ordering::SeqCst) {
            weave(shared, config);
        } else if shared.stop.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn weave(shared: &Shared, config: &Config) {
    let mut baked: Vec<PathBuf> = Vec::new();
    for index in 0..shared.cells.len() {
        let (_, _, path) = cell_paths(config, index);
        // Empty ledger placeholders mark ocean cells; skip them.
        if path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            baked.push(path);
        }
    }
    if baked.is_empty() {
        return;
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
                Ok(()) => {
                    println!("mapfleet: world LIVE with {} cells", baked.len());
                    // Drop the displaced set immediately: at world scale a
                    // retained .prev is a full extra copy of the map, and
                    // a reader that still has the old shards open keeps
                    // them alive via its file handles anyway.
                    let _ = std::fs::remove_dir_all(&prev_dir);
                }
                Err(err) => eprintln!("mapfleet: swap failed: {err}"),
            }
        }
        other => eprintln!("mapfleet: weave failed: {other:?}"),
    }
}
