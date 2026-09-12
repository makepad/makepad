//! Outside-in latency probe for DocumentWorker notify bursts.
//!
//! The worker exposes no per-stage timestamps. This test measures notify →
//! snapshot arrival from the public API, first vs last, inter-arrival batching
//! (to see the snapshot-channel / recv_timeout cadence), and process CPU via
//! getrusage when available.
//!
//! Run:
//!   cargo test --release -p makepad-workspace --test document_worker_latency -- --ignored --nocapture

use makepad_workspace::document_worker::{DocumentWorker, FileSnapshot, MAX_DOCUMENTS};
use makepad_workspace::makepad_widgets::makepad_platform::makepad_network::install_ui_waker;
use makepad_workspace::makepad_widgets::Cx;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Headless tests must not let the platform waker create NSApplication off-thread.
fn headless_cx() -> Cx {
    let cx = Cx::new(Box::new(|_, _| {}));
    install_ui_waker(None);
    cx
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "studio-doc-latency-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self {
            path: std::fs::canonicalize(path).unwrap(),
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn percentile_ms(mut samples: Vec<f64>, p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let idx = ((samples.len() as f64 - 1.0) * p).round() as usize;
    samples[idx.min(samples.len() - 1)]
}

fn collect_until(
    worker: &mut DocumentWorker,
    timeout: Duration,
    mut done: impl FnMut(&[Arc<FileSnapshot>]) -> bool,
) -> (Vec<Arc<FileSnapshot>>, BTreeMap<PathBuf, Instant>) {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut first_seen = BTreeMap::new();
    while Instant::now() < deadline {
        let batch = worker.poll();
        if !batch.is_empty() {
            let now = Instant::now();
            for snap in &batch {
                first_seen
                    .entry(snap.requested_path.clone())
                    .or_insert(now);
            }
            out.extend(batch);
        }
        if done(&out) {
            break;
        }
        // 1 ms empty-poll sleep: coarse enough to keep the test thread from
        // dominating getrusage, fine enough to resolve a 50 ms send cadence.
        std::thread::sleep(Duration::from_millis(1));
    }
    (out, first_seen)
}

/// Process-wide user+sys CPU in milliseconds. RUSAGE_THREAD is not portable
/// (absent on Darwin), so this includes the test thread's poll loop.
#[cfg(unix)]
fn process_cpu_ms() -> Option<f64> {
    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct Timeval {
        tv_sec: std::os::raw::c_long,
        tv_usec: std::os::raw::c_int,
    }
    #[cfg(not(target_os = "macos"))]
    #[repr(C)]
    struct Timeval {
        tv_sec: std::os::raw::c_long,
        tv_usec: std::os::raw::c_long,
    }
    #[repr(C)]
    struct Rusage {
        ru_utime: Timeval,
        ru_stime: Timeval,
        _pad: [u64; 32],
    }
    fn timeval_ms(t: Timeval) -> f64 {
        t.tv_sec as f64 * 1000.0 + t.tv_usec as f64 / 1000.0
    }
    unsafe {
        extern "C" {
            fn getrusage(who: i32, usage: *mut Rusage) -> i32;
        }
        const RUSAGE_SELF: i32 = 0;
        let mut usage = std::mem::zeroed::<Rusage>();
        if getrusage(RUSAGE_SELF, &mut usage) != 0 {
            return None;
        }
        Some(timeval_ms(usage.ru_utime) + timeval_ms(usage.ru_stime))
    }
}

#[cfg(not(unix))]
fn process_cpu_ms() -> Option<f64> {
    None
}

#[derive(Clone, Debug)]
struct BurstRow {
    notifies: usize,
    expected_snaps: usize,
    got_snaps: usize,
    storm_ms: f64,
    first_ms: f64,
    last_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    wall_ms: f64,
    cpu_ms: Option<f64>,
    batch_sizes: Vec<usize>,
    batch_gaps_ms: Vec<f64>,
    latencies: Vec<f64>,
}

fn cluster_batches(mut arrivals_ms: Vec<f64>, gap_ms: f64) -> (Vec<usize>, Vec<f64>) {
    arrivals_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if arrivals_ms.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut sizes = Vec::new();
    let mut gaps = Vec::new();
    let mut size = 1usize;
    let mut prev = arrivals_ms[0];
    gaps.push(0.0);
    for &t in arrivals_ms.iter().skip(1) {
        if t - prev > gap_ms {
            sizes.push(size);
            gaps.push(t - prev);
            size = 1;
        } else {
            size += 1;
        }
        prev = t;
    }
    sizes.push(size);
    (sizes, gaps)
}

fn run_burst(
    worker: &mut DocumentWorker,
    paths: &[PathBuf],
    watched_n: usize,
    notifies: usize,
    tag: &str,
) -> BurstRow {
    let expected = notifies.min(watched_n);
    for (i, path) in paths.iter().take(expected).enumerate() {
        std::fs::write(path, format!("{tag}-{i}\n")).unwrap();
    }

    let cpu0 = process_cpu_ms();
    let storm_start = Instant::now();
    let mut first_notify: BTreeMap<PathBuf, Instant> = BTreeMap::new();
    for i in 0..notifies {
        let path = &paths[i];
        let t = Instant::now();
        worker.notify_changed(path.clone(), 7, 1000 + i as u64).unwrap();
        first_notify.entry(path.clone()).or_insert(t);
        if i + 1 < notifies {
            let target = storm_start + Duration::from_millis(100) * (i as u32 + 1) / notifies as u32;
            let now = Instant::now();
            if target > now {
                std::thread::sleep(target - now);
            }
        }
    }
    let storm_ms = storm_start.elapsed().as_secs_f64() * 1000.0;

    let (after, first_seen) = collect_until(worker, Duration::from_secs(5), |snaps| {
        snaps
            .iter()
            .filter(|s| s.observed.is_some())
            .map(|s| s.requested_path.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == expected
    });
    let wall_ms = storm_start.elapsed().as_secs_f64() * 1000.0;
    let cpu_ms = match (cpu0, process_cpu_ms()) {
        (Some(a), Some(b)) => Some((b - a).max(0.0)),
        _ => None,
    };

    let mut observed: BTreeMap<PathBuf, Vec<Arc<FileSnapshot>>> = BTreeMap::new();
    for snap in &after {
        if snap.observed.is_some() {
            observed
                .entry(snap.requested_path.clone())
                .or_default()
                .push(snap.clone());
        }
    }

    let mut latencies = Vec::new();
    let mut arrivals = Vec::new();
    for path in observed.keys() {
        if let (Some(t0), Some(seen)) = (first_notify.get(path), first_seen.get(path)) {
            let ms = seen.saturating_duration_since(*t0).as_secs_f64() * 1000.0;
            latencies.push(ms);
            arrivals.push(seen.saturating_duration_since(storm_start).as_secs_f64() * 1000.0);
        }
    }

    let (batch_sizes, batch_gaps_ms) = cluster_batches(arrivals, 5.0);
    let first_ms = latencies.iter().copied().fold(f64::INFINITY, f64::min);
    let last_ms = latencies.iter().copied().fold(0.0, f64::max);
    BurstRow {
        notifies,
        expected_snaps: expected,
        got_snaps: observed.len(),
        storm_ms,
        first_ms: if first_ms.is_finite() { first_ms } else { 0.0 },
        last_ms,
        p50_ms: percentile_ms(latencies.clone(), 0.50),
        p95_ms: percentile_ms(latencies.clone(), 0.95),
        max_ms: last_ms,
        wall_ms,
        cpu_ms,
        batch_sizes,
        batch_gaps_ms,
        latencies,
    }
}

fn print_row(row: &BurstRow) {
    let cpu = row
        .cpu_ms
        .map(|c| format!("{c:.2}"))
        .unwrap_or_else(|| "n/a".into());
    let ratio = match row.cpu_ms {
        Some(c) if row.wall_ms > 0.0 => format!("{:.2}", c / row.wall_ms),
        _ => "n/a".into(),
    };
    eprintln!(
        "record=document_notify_latency notifies={} expected={} got={} storm_ms={:.2} first_ms={:.2} last_ms={:.2} p50_ms={:.2} p95_ms={:.2} max_ms={:.2} wall_ms={:.2} cpu_ms={} cpu/wall={} batches={:?} gaps_ms={:?}",
        row.notifies,
        row.expected_snaps,
        row.got_snaps,
        row.storm_ms,
        row.first_ms,
        row.last_ms,
        row.p50_ms,
        row.p95_ms,
        row.max_ms,
        row.wall_ms,
        cpu,
        ratio,
        row.batch_sizes,
        row.batch_gaps_ms.iter().map(|g| format!("{g:.1}")).collect::<Vec<_>>(),
    );
}

fn print_stage_breakdown(row: &BurstRow) {
    eprintln!();
    eprintln!("=== stage breakdown ({} notifies, {} watched files expected) ===", row.notifies, row.expected_snaps);
    eprintln!(
        "  notify storm spread:     {:>8.2} ms   (target 100 ms for burst>1)",
        row.storm_ms
    );
    eprintln!(
        "  first snapshot arrival:  {:>8.2} ms   (notify[path] → first poll that sees it)",
        row.first_ms
    );
    eprintln!(
        "  last snapshot arrival:   {:>8.2} ms   (slowest per-file notify → arrival)",
        row.last_ms
    );
    eprintln!(
        "  p50 / p95 / max:         {:>8.2} / {:>8.2} / {:>8.2} ms",
        row.p50_ms, row.p95_ms, row.max_ms
    );
    eprintln!(
        "  wall (storm+collect):    {:>8.2} ms",
        row.wall_ms
    );
    match row.cpu_ms {
        Some(c) => eprintln!(
            "  process CPU (getrusage): {:>8.2} ms   cpu/wall={:.2}  (process-wide; no RUSAGE_THREAD on Darwin)",
            c,
            if row.wall_ms > 0.0 { c / row.wall_ms } else { 0.0 }
        ),
        None => eprintln!("  process CPU (getrusage):      n/a"),
    }
    eprintln!(
        "  arrival batches (>5 ms gap): sizes={:?}  gaps_ms={:?}",
        row.batch_sizes,
        row.batch_gaps_ms
            .iter()
            .map(|g| format!("{g:.1}"))
            .collect::<Vec<_>>()
    );
    let mut sorted = row.latencies.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let shown: Vec<String> = sorted.iter().map(|v| format!("{v:.0}")).collect();
    eprintln!("  per-file latencies_ms: {shown:?}");

    // Inferences from public-API timings only.
    let mean_gap = if row.batch_gaps_ms.len() > 1 {
        row.batch_gaps_ms.iter().skip(1).sum::<f64>() / (row.batch_gaps_ms.len() - 1) as f64
    } else {
        0.0
    };
    let all_fours = !row.batch_sizes.is_empty()
        && row.batch_sizes.iter().all(|&s| s <= 4)
        && row.batch_sizes.iter().filter(|&&s| s == 4).count() >= 2;
    let gaps_near_50 = mean_gap >= 35.0 && mean_gap <= 70.0;
    let first_near_retry = row.first_ms >= 250.0; // 16 cmds × 20 ms ENOENT retry
    let cpu_low = row.cpu_ms.map(|c| c < row.wall_ms * 0.35).unwrap_or(false);

    eprintln!("  inference:");
    if first_near_retry {
        eprintln!(
            "    first_ms={:.0} looks like the 20 ms ENOENT retry serialised across the first command drain (up to 16).",
            row.first_ms
        );
    } else {
        eprintln!(
            "    first_ms={:.1} is far below 16×20 ms, so the ENOENT retry is not firing on these existing paths.",
            row.first_ms
        );
    }
    if all_fours && gaps_near_50 {
        eprintln!(
            "    snapshots arrive in groups of ≤4, ~{mean_gap:.0} ms apart → snapshot channel depth 4 + worker recv_timeout(50 ms) while pending sends remain."
        );
    } else {
        eprintln!(
            "    batch pattern sizes={:?} mean_gap={:.1} ms (expected ≤4-wide / ~50 ms if channel+timeout dominate).",
            row.batch_sizes, mean_gap
        );
    }
    if cpu_low {
        eprintln!(
            "    cpu/wall is low → the worker is sleeping, not hashing or doing serial disk I/O for the 447 ms."
        );
    } else if let Some(c) = row.cpu_ms {
        eprintln!(
            "    cpu={c:.1} ms vs wall={:.1} ms → a substantial fraction is on-CPU (reads/hashing/test poll), not just sleep.",
            row.wall_ms
        );
    }
}

#[test]
#[ignore = "timing probe; run with --ignored --nocapture"]
fn notify_burst_latency_table_and_breakdown() {
    let dir = TempDir::new();
    const FILE_COUNT: usize = 200;
    let watched_n = MAX_DOCUMENTS;
    let mut paths = Vec::with_capacity(FILE_COUNT);
    for i in 0..FILE_COUNT {
        let path = dir.path.join(format!("f{i:03}.rs"));
        std::fs::write(&path, format!("v0-{i}\n")).unwrap();
        paths.push(path);
    }

    let cx = headless_cx();
    let mut worker =
        DocumentWorker::start_with_fallback(&cx.thread_spawner(), Duration::from_secs(120))
            .unwrap();
    for path in paths.iter().take(watched_n) {
        worker.watch(path.clone()).unwrap();
    }
    let (initial, _) = collect_until(&mut worker, Duration::from_secs(5), |snaps| {
        snaps
            .iter()
            .map(|s| s.requested_path.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == watched_n
    });
    let initial_n = initial
        .iter()
        .map(|s| s.requested_path.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    assert_eq!(initial_n, watched_n, "initial snapshots missing");

    let bursts = [1usize, 8, 32, 200];
    let mut rows = Vec::new();
    for (round, &n) in bursts.iter().enumerate() {
        // Drain anything left from the previous round so observed-seq snapshots
        // in this burst are attributable to these notifies.
        let _ = worker.poll();
        let row = run_burst(&mut worker, &paths, watched_n, n, &format!("b{round}"));
        assert_eq!(
            row.got_snaps, row.expected_snaps,
            "burst {n}: expected {} unique observed snapshots, got {}",
            row.expected_snaps, row.got_snaps
        );
        print_row(&row);
        if n == 200 {
            print_stage_breakdown(&row);
        }
        rows.push(row);
    }

    eprintln!();
    eprintln!("=== latency vs burst size (32 watched, notifies spread over 100 ms) ===");
    eprintln!(
        "| notifies | snapshots | first_ms | last_ms | p50_ms | p95_ms | max_ms | wall_ms | cpu_ms | batches |"
    );
    eprintln!(
        "|---------:|----------:|---------:|--------:|-------:|-------:|-------:|--------:|-------:|---------|"
    );
    for row in &rows {
        let cpu = row
            .cpu_ms
            .map(|c| format!("{c:.1}"))
            .unwrap_or_else(|| "n/a".into());
        let batches = format!("{:?}", row.batch_sizes);
        eprintln!(
            "| {:>8} | {:>9} | {:>8.1} | {:>7.1} | {:>6.1} | {:>6.1} | {:>6.1} | {:>7.1} | {:>6} | {} |",
            row.notifies,
            row.got_snaps,
            row.first_ms,
            row.last_ms,
            row.p50_ms,
            row.p95_ms,
            row.max_ms,
            row.wall_ms,
            cpu,
            batches
        );
    }
    eprintln!();
    eprintln!("live-mode budget: event→arrival p95 ≤ 250 ms");
    if let Some(row) = rows.iter().find(|r| r.notifies == 200) {
        eprintln!(
            "200-notify p95={:.1} ms  first={:.1} ms  last={:.1} ms  {}",
            row.p95_ms,
            row.first_ms,
            row.last_ms,
            if row.p95_ms <= 250.0 {
                "WITHIN budget"
            } else {
                "OVER budget"
            }
        );
    }

    worker.request_stop();
}
