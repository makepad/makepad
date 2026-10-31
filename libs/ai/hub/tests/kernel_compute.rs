//! The `kernel` domain end to end: a loopback node, its HTTP routes and the
//! blocking client. Tiles from the node must be the bits the same kernel
//! gives in-process, on the native backend and the interpreter.

use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::kernel_client::KernelNode;
use makepad_ai_hub::kernel_compute::{compile, PreparedJob};
use makepad_ai_hub::kernel_wire::kernels::{fractal_job, fractal_tile, FractalParams};
use makepad_ai_hub::kernel_wire::*;
use makepad_ai_hub::registry::Registry;
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_script_compute::Backend;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// One node for the whole test binary (the service holds a process lock).
fn node() -> KernelNode {
    static URL: OnceLock<String> = OnceLock::new();
    let url = URL.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("makepad-ai-hub-kernel-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let handle = start_service(ServiceConfig {
            host: "127.0.0.1".to_string(),
            port: 0,
            cache_dir: dir,
            registry: Registry::embedded().unwrap(),
            downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
            peer: makepad_ai_hub::peer_serve::PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
            fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
        })
        .unwrap();
        let url = format!("http://{}", handle.addr);
        std::mem::forget(handle);
        url
    });
    KernelNode::new(url)
}

fn local(job: &KernelJobJson, backend: Backend) -> Vec<TileResult> {
    let kernel = compile(&job.source, backend).unwrap();
    let prepared = PreparedJob::new(kernel, job).unwrap();
    (0..prepared.tile_count()).map(|k| prepared.run_tile(k, backend)).collect()
}

fn by_id(mut tiles: Vec<TileResult>) -> Vec<TileResult> {
    tiles.sort_by_key(|t| t.id);
    tiles
}

fn same_bits(a: &[TileResult], b: &[TileResult]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.id, y.id);
        assert_eq!(x.status, STATUS_OK, "{}", x.message);
        assert_eq!(y.status, STATUS_OK, "{}", y.message);
        assert_eq!(x.work, y.work, "tile {}: counted work", x.id);
        assert_eq!(x.outputs, y.outputs, "tile {}", x.id);
        assert_eq!(x.reduced.iter().map(|r| r.to_bits()).collect::<Vec<_>>(), y.reduced.iter().map(|r| r.to_bits()).collect::<Vec<_>>());
    }
}

#[test]
fn node_fractal_tiles_are_the_local_bits_for_every_kind() {
    let node = node();
    let info = node.info().unwrap();
    assert!(info.cores >= 1);
    for (kind, de, trap) in (0..5).flat_map(|k| [(k, false, false), (k, true, false), (k, false, true), (k, true, true)]).chain([(5, false, false)]) {
        let p = FractalParams { kind, max_iter: 300, de, trap, ..FractalParams::default() };
        let tiles = (0..6u64).map(|k| fractal_tile(k, -2.0 + 0.6 * k as f64, -0.4, 0.8, 32, &p)).collect();
        let job = fractal_job(&p, tiles);
        let remote = by_id(node.run(&job, 32 * 32).unwrap());
        let native = local(&job, Backend::Native);
        let interp = local(&job, Backend::Interp);
        same_bits(&remote, &native);
        same_bits(&native, &interp);
        // a texel inside the set and one outside
        if kind == 5 {
            // near 1 the cubic converges to root 0 (class 1); some texel reaches each root
            let t = remote.iter().find(|t| t.id == 3).unwrap().output("out").unwrap();
            let classes: std::collections::HashSet<u32> = t.iter().map(|w| w >> 24).collect();
            assert!((1..=3).all(|c| classes.contains(&c)), "{classes:?}");
        }
        if kind == 0 && de {
            // escaped texels carry distance codes (top byte 1..254)
            let t = remote.iter().find(|t| t.id == 3).unwrap().output("out").unwrap();
            assert!(t.iter().any(|w| (1..255).contains(&(w >> 24))));
        }
        if kind == 0 && !de && !trap {
            let t0 = remote.iter().find(|t| t.id == 3).unwrap().output("out").unwrap().to_vec();
            assert!(t0.iter().any(|w| w >> 24 == 255) && t0.iter().any(|w| w >> 24 == 0));
        }
    }
}

/// A second, unrelated kernel: a map over an f32 input with a per-tile
/// param and the seed, and a reduce. Nothing in the node knows fractals.
#[test]
fn any_kernel_runs_maps_and_reduces() {
    let node = node();
    let src: Vec<f32> = (0..1000).map(|i| i as f32 * 0.5).collect();
    let map = KernelJobJson {
        source: "let src = input(f32)\nlet out = output(vec2)\nlet scale = param(1.0, -100, 100)\nfn element(i) { out[i] = vec2(src[i] * scale, float(i + seed)) }".into(),
        params: vec![KernelParamJson { name: "scale".into(), value: 2.0 }],
        inputs: vec![KernelBufferJson::from_f32("src", &src)],
        tiles: vec![
            KernelTileJson { id: 7, count: 1000, params: vec![], inputs: vec![] },
            KernelTileJson { id: 8, count: 10, params: vec![KernelParamJson { name: "scale".into(), value: -1.0 }], inputs: vec![] },
        ],
        encoding: "raw".into(),
        seed: 5,
        passes: None,
        feedback: None,
    };
    let remote = by_id(node.run(&map, 2000).unwrap());
    same_bits(&remote, &local(&map, Backend::Native));
    let a = remote[0].output("out").unwrap();
    assert_eq!(a.len(), 2000);
    assert_eq!((f32::from_bits(a[2 * 10]), f32::from_bits(a[2 * 10 + 1])), (10.0, 15.0));
    let b = remote[1].output("out").unwrap();
    assert_eq!(b.len(), 20);
    assert_eq!(f32::from_bits(b[2 * 3]), -1.5);

    let reduce = KernelJobJson {
        source: "let v = input(f32)\nfn reduce_sum(i) { v[i] * v[i] }".into(),
        params: vec![],
        inputs: vec![],
        tiles: (0..3u64)
            .map(|k| KernelTileJson { id: k, count: 100, params: vec![], inputs: vec![KernelBufferJson::from_f32("v", &vec![k as f32 + 1.0; 100])] })
            .collect(),
        encoding: "delta-deflate".into(),
        seed: 0,
        passes: None,
        feedback: None,
    };
    let remote = by_id(node.run(&reduce, 0).unwrap());
    same_bits(&remote, &local(&reduce, Backend::Native));
    assert_eq!(remote.iter().map(|t| t.reduced[0]).collect::<Vec<_>>(), vec![100.0, 400.0, 900.0]);
}

#[test]
fn cancel_drops_queued_tiles_and_stops_running_ones() {
    let node = node();
    let cores = node.info().unwrap().cores as usize;
    // every texel inside the set at the cap: the slowest tiles there are
    let p = FractalParams { kind: 0, max_iter: 16384, ..FractalParams::default() };
    let n = (cores * 4).max(8);
    let tiles = (0..n as u64).map(|k| fractal_tile(k, -1.37, -0.02, 0.04, 128, &p)).collect();
    let job = fractal_job(&p, tiles);
    let submitted = node.submit(&job).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let t0 = Instant::now();
    node.cancel(submitted.job_id).unwrap();
    let mut got = Vec::new();
    loop {
        let batch = node.poll(submitted.job_id, Duration::from_millis(2000), 128 * 128).unwrap();
        got.extend(batch.tiles);
        if batch.done {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(5), "a cancelled job must end promptly");
    }
    let stopped = t0.elapsed();
    let ok = got.iter().filter(|t| t.status == STATUS_OK).count();
    let cancelled = got.iter().filter(|t| t.status == STATUS_CANCELLED).count();
    eprintln!("cancel: {n} tiles, {ok} finished, {cancelled} stopped while running, {} never ran; ended {stopped:?} after the cancel", n - got.len());
    assert!(got.len() < n, "queued tiles must not run after a cancel");
    assert!(stopped < Duration::from_secs(2));
    // the job is gone
    assert!(node.poll(submitted.job_id, Duration::ZERO, 16).is_err());
}

#[test]
fn bad_jobs_are_refused_with_the_reason() {
    let node = node();
    let mut job = fractal_job(&FractalParams::default(), vec![fractal_tile(0, 0.0, 0.0, 1.0, 8, &FractalParams::default())]);
    job.source = "let out = output(u32)\nfn element(i) { out[i] = nope }".into();
    let e = node.submit(&job).unwrap_err().to_string();
    assert!(e.contains("does not compile") && e.contains("2:"), "{e}");

    let mut job = fractal_job(&FractalParams::default(), vec![fractal_tile(0, 0.0, 0.0, 1.0, 8, &FractalParams::default())]);
    job.params.push(KernelParamJson { name: "nope".into(), value: 1.0 });
    assert!(node.submit(&job).unwrap_err().to_string().contains("no param `nope`"));

    let mut job = fractal_job(&FractalParams::default(), vec![fractal_tile(0, 0.0, 0.0, 1.0, 8, &FractalParams::default())]);
    job.tiles[0].inputs.clear();
    assert!(node.submit(&job).unwrap_err().to_string().contains("input `view` is not bound"));

    let emit = KernelJobJson {
        source: "let seg = emit_buffer(2, 4)\nfn primitive(i) { emit(seg, vec2(1.0, 2.0)) }".into(),
        params: vec![],
        inputs: vec![],
        tiles: vec![KernelTileJson { id: 0, count: 4, params: vec![], inputs: vec![] }],
        encoding: "raw".into(),
        seed: 0,
        passes: None,
        feedback: None,
    };
    let e = node.submit(&emit).unwrap_err().to_string();
    assert!(e.contains("not served") || e.contains("does not compile"), "{e}");

    let mut job = fractal_job(&FractalParams::default(), vec![fractal_tile(0, 0.0, 0.0, 1.0, 8, &FractalParams::default())]);
    job.tiles[0].count = (MAX_TILE_ELEMENTS + 1) as u32;
    assert!(node.submit(&job).unwrap_err().to_string().contains("elements"));
}

/// The stream (one websocket per node): the same tiles as by request,
/// pushed as they finish, and a cancel up the same stream.
#[test]
fn the_stream_pushes_the_same_tiles_and_cancels() {
    use makepad_ai_hub::kernel_client::KernelStream;
    let node = node();
    let stream = KernelStream::open(&node).unwrap();
    let p = FractalParams { kind: 2, max_iter: 400, ..FractalParams::default() };
    let job = fractal_job(&p, (0..8u64).map(|k| fractal_tile(k, -2.0 + 0.4 * k as f64, -1.0, 0.5, 48, &p)).collect());
    let submitted = stream.submit(&job).unwrap();
    let mut got = Vec::new();
    loop {
        let batch = stream.next_batch(submitted.job_id, Duration::from_secs(5)).unwrap();
        got.extend(batch.tiles);
        if batch.done {
            break;
        }
    }
    same_bits(&by_id(got), &by_id(node.run(&job, 48 * 48).unwrap()));

    let heavy = FractalParams { kind: 0, max_iter: 16384, ..FractalParams::default() };
    let job = fractal_job(&heavy, (0..32u64).map(|k| fractal_tile(k, -1.37, -0.02, 0.04, 128, &heavy)).collect());
    let submitted = stream.submit(&job).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let t0 = Instant::now();
    stream.cancel(submitted.job_id);
    let mut n = 0;
    loop {
        let batch = stream.next_batch(submitted.job_id, Duration::from_secs(5)).unwrap();
        n += batch.tiles.len();
        if batch.done {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(3), "a cancelled stream job must end promptly");
    }
    assert!(n < 32);
    // a bad job is refused through the stream with the reason
    let mut bad = job.clone();
    bad.source = "nope".into();
    assert!(stream.submit(&bad).unwrap_err().to_string().contains("does not compile"));
}

/// The fleet over two "nodes" (the loopback node twice): every tag comes
/// back exactly once, with the local bits, even when the frame's tail is
/// copied to the idle node.
#[test]
fn the_fleet_answers_every_tile_once_with_the_local_bits() {
    use makepad_ai_hub::kernel_client::{Batch, KernelFleet};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    let node = node();
    let info = node.info().unwrap();
    let p = FractalParams { kind: 0, max_iter: 3000, ..FractalParams::default() };
    // cheap tiles and a few dear ones (inside the set at the cap)
    let tiles: Vec<KernelTileJson> = (0..48u64)
        .map(|k| if k % 12 == 11 { fractal_tile(k, -0.3, -0.05, 0.1, 64, &p) } else { fractal_tile(k, 0.3 + 0.01 * k as f64, 0.5, 0.05, 64, &p) })
        .collect();
    let want: HashMap<u64, Vec<u32>> = local(&fractal_job(&p, tiles.clone()), Backend::Native).into_iter().map(|t| (t.id, t.output("out").unwrap().to_vec())).collect();
    let queue = Arc::new(Mutex::new(tiles.into_iter().collect::<VecDeque<_>>()));
    let got: Arc<Mutex<Vec<(u64, TileResult)>>> = Arc::new(Mutex::new(Vec::new()));
    let source = {
        let queue = queue.clone();
        Arc::new(move |max: usize| {
            let mut q = queue.lock().unwrap();
            let take: Vec<KernelTileJson> = (0..max.min(q.len())).filter_map(|_| q.pop_front()).collect();
            if take.is_empty() {
                return None;
            }
            let tags = take.iter().map(|t| t.id).collect();
            Some(Batch { job: fractal_job(&p, take), tags })
        })
    };
    let sink = {
        let got = got.clone();
        Arc::new(move |_node: usize, tag: u64, r: TileResult| got.lock().unwrap().push((tag, r)))
    };
    let fleet = KernelFleet::start(vec![(node.clone(), info.clone()), (node.clone(), info)], source, sink);
    let t0 = Instant::now();
    while got.lock().unwrap().len() < 48 {
        assert!(t0.elapsed() < Duration::from_secs(60), "the fleet must answer every tile");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(300));
    drop(fleet);
    let got = got.lock().unwrap();
    assert_eq!(got.len(), 48, "exactly once");
    for (tag, r) in got.iter() {
        assert_eq!(r.status, STATUS_OK, "{}", r.message);
        assert_eq!(r.output("out").unwrap(), want[tag].as_slice(), "tile {tag}");
    }
}

/// Long iteration counts run in passes with the orbit state fed back on the
/// node: a 2-pass long render equals the one-call render at the same
/// iteration count, and counts past one call's bound render (and take longer).
#[test]
fn long_iteration_counts_run_in_passes_with_the_same_bits() {
    use makepad_ai_hub::kernel_wire::kernels::FRACTAL_LONG;
    let node = node();
    for (kind, de, trap) in [(0, false, false), (0, true, false), (1, false, true), (2, true, true), (3, false, false)] {
        let p = FractalParams { kind, max_iter: 3000, de, trap, ..FractalParams::default() };
        let tiles: Vec<KernelTileJson> = (0..4u64).map(|k| fractal_tile(k, -1.9 + 0.5 * k as f64, -0.3, 0.6, 32, &p)).collect();
        let one = fractal_job(&p, tiles.clone());
        // the same count through the long kernel: 2 passes
        let mut long = fractal_job(&FractalParams { max_iter: 20000, ..p }, tiles);
        assert_eq!(long.source, FRACTAL_LONG);
        long.params.retain(|q| q.name != "max_iter" && q.name != "passes");
        long.params.push(KernelParamJson { name: "max_iter".into(), value: 3000.0 });
        long.params.push(KernelParamJson { name: "passes".into(), value: 2.0 });
        long.passes = Some(2);
        let a = by_id(node.run(&one, 32 * 32).unwrap());
        let b = by_id(node.run(&long, 32 * 32).unwrap());
        same_bits_outputs(&a, &b);
        same_bits(&b, &local(&long, Backend::Native));
        assert!(b.iter().all(|t| t.outputs.len() == 1), "fed-back state stays on the node");
    }
    // past the one-call cap: 40000 iterations inside the set
    let p = FractalParams { kind: 0, max_iter: 40000, ..FractalParams::default() };
    let job = fractal_job(&p, vec![fractal_tile(0, -0.3, -0.05, 0.1, 16, &p)]);
    assert_eq!(job.passes, Some(20));
    let t = node.run(&job, 16 * 16).unwrap();
    assert_eq!(t[0].status, STATUS_OK, "{}", t[0].message);
    assert!(t[0].output("out").unwrap().iter().all(|w| w >> 24 == 255));
}

fn same_bits_outputs(a: &[TileResult], b: &[TileResult]) {
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(b) {
        assert_eq!((x.status, y.status), (STATUS_OK, STATUS_OK), "{} {}", x.message, y.message);
        assert_eq!(x.output("out"), y.output("out"), "tile {}", x.id);
    }
}
