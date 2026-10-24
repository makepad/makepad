//! kernel-bench: the `kernel` domain's render gate and perf table.
//!
//! ```text
//! kernel-bench [--nodes host:port,...] [--local N] [--frame WxH] [--tile PX]
//!              [--iter N] [--at X,Y,WIDTH] [--kind K] [--cancel] [--skip-alone]
//! kernel-bench --status [--nodes ...]   each node's GET /kernel
//! ```
//!
//! Renders one fractal frame (tiles in a spiral from the centre) four ways:
//! on N threads of this machine (the reference), on each kernel node alone,
//! and on all nodes plus the local threads together; every remote tile is
//! bit-compared with the reference. `--cancel` then measures how long the
//! nodes take to go quiet after a frame is cancelled mid-render.
//! Nodes: `--nodes` (fleet `host:port`, or `http://` loopback), else
//! `MAKEPAD_AI_KERNEL_NODES` and the LAN beacon.

use makepad_ai_hub::kernel_client::{kernel_nodes, Batch, KernelFleet, KernelNode};
use makepad_ai_hub::kernel_compute::{compile, PreparedJob};
use makepad_ai_hub::kernel_wire::kernels::{fractal_job, fractal_tile, FractalParams};
use makepad_ai_hub::kernel_wire::*;
use makepad_script_compute::Backend;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Opts {
    nodes: Option<String>,
    local: usize,
    width: u32,
    height: u32,
    tile: u32,
    params: FractalParams,
    at: (f64, f64, f64),
    cancel: bool,
    skip_alone: bool,
    status: bool,
    rtt: bool,
}

fn main() {
    let mut o = Opts {
        nodes: None,
        local: std::thread::available_parallelism().map_or(1, |n| n.get()),
        width: 1920,
        height: 1080,
        tile: 256,
        params: FractalParams { max_iter: 2000, ..FractalParams::default() },
        // the seahorse valley
        at: (-0.743_643_9, 0.131_825_9, 0.02),
        cancel: false,
        skip_alone: false,
        status: false,
        rtt: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| die(&format!("{a} needs a value")));
        match a.as_str() {
            "--nodes" => o.nodes = Some(val()),
            "--local" => o.local = val().parse().unwrap_or_else(|_| die("--local N")),
            "--frame" => {
                let v = val();
                let (w, h) = v.split_once('x').unwrap_or_else(|| die("--frame WxH"));
                o.width = w.parse().unwrap_or_else(|_| die("--frame WxH"));
                o.height = h.parse().unwrap_or_else(|_| die("--frame WxH"));
            }
            "--tile" => o.tile = val().parse().unwrap_or_else(|_| die("--tile PX")),
            "--iter" => o.params.max_iter = val().parse().unwrap_or_else(|_| die("--iter N")),
            "--kind" => o.params.kind = val().parse().unwrap_or_else(|_| die("--kind K")),
            "--at" => {
                let v: Vec<f64> = val().split(',').map(|x| x.parse().unwrap_or_else(|_| die("--at X,Y,WIDTH"))).collect();
                if v.len() != 3 {
                    die("--at X,Y,WIDTH");
                }
                o.at = (v[0], v[1], v[2]);
            }
            "--cancel" => o.cancel = true,
            "--skip-alone" => o.skip_alone = true,
            "--status" => o.status = true,
            "--rtt" => o.rtt = true,
            _ => die(&format!("unknown argument {a}")),
        }
    }
    if let Some(list) = &o.nodes {
        std::env::set_var("MAKEPAD_AI_KERNEL_NODES", list);
    } else {
        // the beacon needs a moment to be heard
        let _ = kernel_nodes();
        std::thread::sleep(Duration::from_millis(2500));
    }
    let nodes = kernel_nodes();
    if nodes.is_empty() {
        eprintln!("no kernel nodes found (pass --nodes host:port)");
    }
    if o.rtt {
        rtt(&nodes);
        return;
    }
    if o.status {
        println!("| node | api | arch | backend | cores | jobs | queued | tiles done | busy s |");
        println!("|---|---:|---|---|---:|---:|---:|---:|---:|");
        for (node, i) in &nodes {
            println!("| {} | {} | {} | {} | {} | {} | {} | {} | {:.1} |", node.base_url, i.api, i.arch, i.backend, i.cores, i.jobs, i.tiles_queued, i.tiles_done, i.busy_ms as f64 / 1e3);
        }
        return;
    }

    let tiles = frame_tiles(&o);
    let n = tiles.len();
    println!(
        "frame {}x{} at ({}, {}) width {}, kind {}, max_iter {}: {} tiles of {}px",
        o.width, o.height, o.at.0, o.at.1, o.at.2, o.params.kind, o.params.max_iter, n, o.tile
    );
    let kernel = compile(&kernels_source(&o.params), Backend::Native).unwrap_or_else(|e| die(&e));

    // 1. local reference
    let t0 = Instant::now();
    let reference = render_local(&kernel, &o.params, &tiles, o.local);
    let local_s = t0.elapsed().as_secs_f64();
    let mut rows = vec![("local".to_string(), std::env::consts::ARCH.to_string(), o.local as u32, n, local_s, true)];

    // 2. each node alone
    for (node, info) in nodes.iter().filter(|_| !o.skip_alone) {
        let (got, secs) = render_fleet(&[(node.clone(), info.clone())], &o.params, &tiles, None);
        let same = compare(&reference, &got);
        rows.push((node.base_url.clone(), info.arch.clone(), info.cores, got.len(), secs, same));
    }

    // 3. everything together
    let mut together = None;
    if !nodes.is_empty() {
        let local = Arc::new(LocalPool { kernel: kernel.clone(), threads: o.local });
        let (got, secs) = render_fleet(&nodes, &o.params, &tiles, Some(local.clone()));
        let same = compare(&reference, &got);
        let mut per: HashMap<String, usize> = HashMap::new();
        for (who, _) in got.values() {
            *per.entry(who.clone()).or_default() += 1;
        }
        together = Some((secs, same, per));
    }

    println!();
    println!("| renderer | arch | cores | tiles | seconds | tiles/s | identical to local |");
    println!("|---|---|---:|---:|---:|---:|---|");
    for (who, arch, cores, tiles, secs, same) in &rows {
        println!("| {who} | {arch} | {cores} | {tiles} | {secs:.2} | {:.1} | {} |", *tiles as f64 / secs, if *same { "yes" } else { "NO" });
    }
    if let Some((secs, same, per)) = together {
        let cores: u32 = o.local as u32 + nodes.iter().map(|(_, i)| i.cores).sum::<u32>();
        println!("| all together | - | {cores} | {n} | {secs:.2} | {:.1} | {} |", n as f64 / secs, if same { "yes" } else { "NO" });
        println!();
        println!("speedup vs local only: {:.2}x", local_s / secs);
        let mut per: Vec<_> = per.into_iter().collect();
        per.sort();
        println!();
        println!("| in the all-together run | tiles | tiles/s |");
        println!("|---|---:|---:|");
        for (who, k) in per {
            println!("| {who} | {k} | {:.1} |", k as f64 / secs);
        }
    }
    let all_same = rows.iter().all(|r| r.5);
    if o.cancel && !nodes.is_empty() {
        cancel_phase(&nodes, &o.params, &tiles);
    }
    if !all_same {
        eprintln!("FAIL: a node's tiles differ from the local render");
        std::process::exit(1);
    }
}

/// The kernel `fractal_job` picks for these params.
fn kernels_source(p: &FractalParams) -> String {
    fractal_job(p, Vec::new()).source
}

fn die(msg: &str) -> ! {
    eprintln!("kernel-bench: {msg}");
    std::process::exit(2);
}

/// The frame's tiles, nearest the centre first (rings, then angle).
fn frame_tiles(o: &Opts) -> Vec<KernelTileJson> {
    let px = o.at.2 / o.width as f64;
    let size = px * o.tile as f64;
    let (cols, rows) = (o.width.div_ceil(o.tile) as i64, o.height.div_ceil(o.tile) as i64);
    let x0 = o.at.0 - 0.5 * o.width as f64 * px;
    let y0 = o.at.1 - 0.5 * o.height as f64 * px;
    let mut cells: Vec<(i64, i64)> = (0..rows).flat_map(|j| (0..cols).map(move |i| (i, j))).collect();
    let (ci, cj) = ((cols - 1) as f64 * 0.5, (rows - 1) as f64 * 0.5);
    cells.sort_by(|a, b| {
        let ra = (a.0 as f64 - ci).abs().max((a.1 as f64 - cj).abs());
        let rb = (b.0 as f64 - ci).abs().max((b.1 as f64 - cj).abs());
        ra.partial_cmp(&rb).unwrap().then_with(|| {
            let aa = (a.1 as f64 - cj).atan2(a.0 as f64 - ci);
            let ab = (b.1 as f64 - cj).atan2(b.0 as f64 - ci);
            aa.partial_cmp(&ab).unwrap()
        })
    });
    cells
        .into_iter()
        .enumerate()
        .map(|(k, (i, j))| fractal_tile(k as u64, x0 + i as f64 * size, y0 + j as f64 * size, size, o.tile, &o.params))
        .collect()
}

struct LocalPool {
    kernel: makepad_script_compute::kernel::KernelRef,
    threads: usize,
}

fn render_local(kernel: &makepad_script_compute::kernel::KernelRef, p: &FractalParams, tiles: &[KernelTileJson], threads: usize) -> HashMap<u64, Vec<u32>> {
    let queue = Mutex::new(tiles.iter().cloned().collect::<VecDeque<_>>());
    let out = Mutex::new(HashMap::new());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| loop {
                let Some(tile) = queue.lock().unwrap().pop_front() else { return };
                let r = run_one(kernel, p, tile);
                out.lock().unwrap().insert(r.id, r.output("out").unwrap_or_default().to_vec());
            });
        }
    });
    out.into_inner().unwrap()
}

fn run_one(kernel: &makepad_script_compute::kernel::KernelRef, p: &FractalParams, tile: KernelTileJson) -> TileResult {
    let job = fractal_job(p, vec![tile]);
    let prepared = PreparedJob::new(kernel.clone(), &job).unwrap_or_else(|e| die(&e));
    prepared.run_tile(0, Backend::Native)
}

type Got = HashMap<u64, (String, Vec<u32>)>;

/// All tiles through `nodes` (and the local pool's threads, pulling from
/// the same queue when given); (tile id -> (who, texels), seconds).
fn render_fleet(nodes: &[(KernelNode, KernelInfoJson)], p: &FractalParams, tiles: &[KernelTileJson], local: Option<Arc<LocalPool>>) -> (Got, f64) {
    let queue = Arc::new(Mutex::new(tiles.iter().cloned().collect::<VecDeque<_>>()));
    let got: Arc<Mutex<Got>> = Arc::new(Mutex::new(HashMap::new()));
    let names: Vec<String> = nodes.iter().map(|(n, _)| n.base_url.clone()).collect();
    let total = tiles.len();
    let t0 = Instant::now();
    let source = {
        let queue = queue.clone();
        let p = *p;
        Arc::new(move |max: usize| {
            let mut q = queue.lock().unwrap();
            if q.is_empty() {
                return None;
            }
            let take: Vec<KernelTileJson> = (0..max.min(q.len())).filter_map(|_| q.pop_front()).collect();
            let tags = take.iter().map(|t| t.id).collect();
            Some(Batch { job: fractal_job(&p, take), tags })
        })
    };
    let sink = {
        let got = got.clone();
        let queue = queue.clone();
        let tiles: Vec<KernelTileJson> = tiles.to_vec();
        Arc::new(move |node: usize, id: u64, r: TileResult| {
            if r.status == STATUS_OK {
                got.lock().unwrap().insert(id, (names[node].clone(), r.output("out").unwrap_or_default().to_vec()));
            } else {
                // a failed node's tiles go back to the front of the queue
                eprintln!("tile {id} on {}: {}", names[node], r.message);
                if let Some(t) = tiles.iter().find(|t| t.id == id) {
                    queue.lock().unwrap().push_front(t.clone());
                }
            }
        })
    };
    let fleet = KernelFleet::start(nodes.to_vec(), source, sink);
    std::thread::scope(|s| {
        if let Some(local) = &local {
            for _ in 0..local.threads.max(1) {
                s.spawn(|| loop {
                    let Some(tile) = queue.lock().unwrap().pop_front() else {
                        if got.lock().unwrap().len() >= total {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    };
                    let r = run_one(&local.kernel, p, tile);
                    got.lock().unwrap().insert(r.id, ("local".into(), r.output("out").unwrap_or_default().to_vec()));
                });
            }
        }
        while got.lock().unwrap().len() < total {
            std::thread::sleep(Duration::from_millis(2));
        }
    });
    let secs = t0.elapsed().as_secs_f64();
    for st in fleet.stats() {
        eprintln!("  {} ({} cores, {}): {} tiles, {} tail copies ({} won), {} failures {}", st.url, st.cores, st.arch, st.tiles_done, st.copies, st.won, st.failures, st.last_error.unwrap_or_default());
    }
    drop(fleet);
    let got = Arc::try_unwrap(got).ok().map(|m| m.into_inner().unwrap()).unwrap_or_default();
    (got, secs)
}

fn compare(reference: &HashMap<u64, Vec<u32>>, got: &Got) -> bool {
    let mut same = got.len() == reference.len();
    for (id, (who, words)) in got {
        if reference.get(id) != Some(words) {
            eprintln!("tile {id} from {who} differs from the local render");
            same = false;
        }
    }
    same
}

/// A frame cancelled 300 ms into its render: how long until every node's
/// tiles are back (finished, stopped or never run).
fn cancel_phase(nodes: &[(KernelNode, KernelInfoJson)], p: &FractalParams, tiles: &[KernelTileJson]) {
    let queue = Arc::new(Mutex::new(tiles.iter().cloned().collect::<VecDeque<_>>()));
    let back = Arc::new(Mutex::new((0usize, 0usize, 0usize)));
    let source = {
        let queue = queue.clone();
        let p = *p;
        Arc::new(move |max: usize| {
            let mut q = queue.lock().unwrap();
            if q.is_empty() {
                return None;
            }
            let take: Vec<KernelTileJson> = (0..max.min(q.len())).filter_map(|_| q.pop_front()).collect();
            let tags = take.iter().map(|_| 0u32).collect();
            Some(Batch { job: fractal_job(&p, take), tags })
        })
    };
    let sink = {
        let back = back.clone();
        Arc::new(move |_node: usize, _layer: u32, r: TileResult| {
            let mut b = back.lock().unwrap();
            match r.status {
                STATUS_OK => b.0 += 1,
                STATUS_CANCELLED => b.1 += 1,
                _ => b.2 += 1,
            }
        })
    };
    let fleet = KernelFleet::start(nodes.to_vec(), source, sink);
    std::thread::sleep(Duration::from_millis(300));
    let taken = tiles.len() - {
        let mut q = queue.lock().unwrap();
        let left = q.len();
        q.clear();
        left
    };
    let t0 = Instant::now();
    fleet.cancel(|_| true);
    loop {
        let b = *back.lock().unwrap();
        if b.0 + b.1 + b.2 >= taken {
            break;
        }
        if t0.elapsed() > Duration::from_secs(10) {
            println!("cancel: nodes still busy after 10 s ({b:?} of {taken})");
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let b = *back.lock().unwrap();
    println!("cancel: {taken} tiles in flight; {} finished, {} stopped or dropped, {} failed; all back {:.0} ms after the cancel", b.0, b.1, b.2, t0.elapsed().as_secs_f64() * 1e3);
}

/// One 8x8 tile, 30 times in a row on each node: by request (submit + poll,
/// each a TLS connect) and over the stream.
fn rtt(nodes: &[(KernelNode, KernelInfoJson)]) {
    use makepad_ai_hub::kernel_client::KernelStream;
    let p = FractalParams::default();
    let job = fractal_job(&p, vec![fractal_tile(0, -0.5, 0.0, 0.01, 8, &p)]);
    println!("| node | request ms/job | stream ms/job |");
    println!("|---|---:|---:|");
    for (node, _) in nodes {
        let _ = node.run(&job, 64);
        let t0 = Instant::now();
        for _ in 0..30 {
            node.run(&job, 64).unwrap_or_else(|e| die(&e.to_string()));
        }
        let req = t0.elapsed().as_secs_f64() * 1e3 / 30.0;
        let stream_ms = match KernelStream::open(node) {
            Ok(stream) => {
                let once = |stream: &KernelStream| {
                    let sub = stream.submit(&job).unwrap_or_else(|e| die(&e.to_string()));
                    while !stream.next_batch(sub.job_id, Duration::from_secs(5)).unwrap_or_else(|e| die(&e.to_string())).done {}
                };
                once(&stream);
                let t0 = Instant::now();
                for _ in 0..30 {
                    once(&stream);
                }
                format!("{:.2}", t0.elapsed().as_secs_f64() * 1e3 / 30.0)
            }
            Err(e) => format!("none ({e})"),
        };
        println!("| {} | {req:.2} | {stream_ms} |", node.base_url);
    }
}
