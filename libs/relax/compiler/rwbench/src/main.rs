// Real-world benchmark workloads for Relax (phase 1): the same source under rustc --release and
// `makepad run`. Each workload is `setup() -> iteration closure`; setup (reading inputs) is not
// timed per iteration but counts towards time-to-steady. Usage: relax-rwbench <workload>...
//
// Output per workload (one line):
//   | <name> steady_ms=<f> tts_main_ms=<f> tts_proc_ms=<f> iters=<n> first_ms=<f> <checksum>
// steady_ms   median of the last N iteration times (N = min(10, iterations started after 5 s), at least 3)
// tts_main_ms ms from entering main until the iteration the per-iteration time settles within 10%
//             of steady_ms (3-iteration running median) starts; tts_proc_ms the same measured from
//             the process start (RW_WALL0 from run.py, else /proc on Linux; includes Relax's compile
//             before main; else equal to tts_main_ms)
// Env: RW_MS (run each workload at least this many ms, default 8000), RW_MIN_ITERS (default 20),
//      RW_ROOT (the makepad tree root holding the inputs; default: the tree this crate is in).
//      RW_ITERS (positive count: warm up for RW_MS, then run exactly this many iterations),
//      RW_PERF_CTL=<control fifo>,<ack fifo> (fixed mode only; perf stat --control fifo:...
//      -D -1 --no-inherit counts the main thread only, excluding setup and warm-up).
use std::hint::black_box;
use std::time::Instant;

mod audio;
mod csg;
mod inputs;
mod layout;
mod llm;
mod maps;
mod parsers;
mod script;
mod sdf;
mod text;

fn main() {
    let t0 = Instant::now();
    let proc_ms_at_start = process_age_ms();
    let only: Vec<String> = std::env::args().skip(1).collect();
    let all: [(&str, fn() -> Box<dyn FnMut() -> u64>); 17] = [
        ("text_shape_raster", text::setup),
        ("sdf_glyphs", sdf::setup),
        ("inflate_real", inputs::inflate_setup),
        ("zip_read", inputs::zip_setup),
        ("png_decode", inputs::png_setup),
        ("brotli_decode", inputs::brotli_setup),
        ("maps_tile_decode", maps::setup),
        ("layout_widgets", layout::setup),
        ("llm_host", llm::setup),
        ("rust_tokenize", parsers::rust_tokens_setup),
        ("markdown_parse", parsers::markdown_setup),
        ("html_parse", parsers::html_setup),
        ("piano_render", audio::piano_setup),
        ("ogg_decode", audio::ogg_setup),
        ("audio_mix", audio::mix_setup),
        ("splash_eval", script::setup),
        ("csg_boolean", csg::setup),
    ];
    for (name, setup) in all.iter() {
        if only.is_empty() || only.iter().any(|o| o == name) {
            run(name, *setup, t0, proc_ms_at_start);
        }
    }
}

/// ms since this process started (Linux: /proc/self/stat start time vs /proc/uptime), else None
fn process_age_ms() -> Option<f64> {
    // RW_WALL0=<epoch ns> (set by run.py just before it starts the process, so it includes Relax's
    // compile and startup): exact; else the /proc start time (Linux, 10 ms resolution)
    if let Ok(w) = std::env::var("RW_WALL0") {
        let w0: f64 = w.parse().ok()?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos() as f64;
        return Some((now - w0) / 1e6);
    }
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    let start_ticks: f64 = rest.split(' ').nth(19)?.parse().ok()?;
    let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split(' ').next()?.parse().ok()?;
    Some((up - start_ticks / 100.0) * 1000.0)
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

fn run(name: &str, setup: fn() -> Box<dyn FnMut() -> u64>, t0: Instant, proc_ms_at_start: Option<f64>) {
    let budget_ms: f64 = std::env::var("RW_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(8000.0);
    let min_iters: usize = std::env::var("RW_MIN_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(20);
    let begin = Instant::now();
    let mut iter = setup();
    if let Ok(count) = std::env::var("RW_ITERS") {
        let count: usize = count.parse().expect("RW_ITERS must be a positive integer");
        assert!(count > 0, "RW_ITERS must be positive");
        let expected = black_box(iter());
        let warm = Instant::now();
        let mut warmed = 1;
        while warmed < min_iters || warm.elapsed().as_secs_f64() * 1000.0 < budget_ms {
            assert_eq!(black_box(iter()), expected, "{}: warm-up checksum differs", name);
            warmed += 1;
        }
        // RW_O3_PROGRESS: go on warming until the JIT reports its optimised code settled (5 quiet iterations), so the counted
        // iterations run optimised code only
        if let Ok(path) = std::env::var("RW_O3_PROGRESS") {
            let (mut quiet, mut last) = (0usize, u64::MAX);
            let limit = Instant::now();
            while quiet < 5 && limit.elapsed().as_secs() < 120 {
                assert_eq!(black_box(iter()), expected, "{}: warm-up checksum differs", name);
                let t = std::fs::read_to_string(&path).unwrap_or_default();
                let mut it = t.split_whitespace().map(|x| x.parse::<u64>().ok());
                match (it.next().flatten(), it.next().flatten()) {
                    (Some(0), Some(i)) if i == last => quiet += 1,
                    (_, Some(i)) => {
                        quiet = 0;
                        last = i;
                    }
                    _ => quiet = 0,
                }
            }
        }
        let control = std::env::var("RW_PERF_CTL").ok();
        let perf = |command: &str| {
            if let Some(paths) = &control {
                use std::io::{Read, Write};
                let (ctl, ack) = paths.split_once(',').expect("RW_PERF_CTL requires control,ack paths");
                let mut writer = std::fs::OpenOptions::new().write(true).open(ctl).unwrap();
                writer.write_all(command.as_bytes()).unwrap();
                let mut reader = std::fs::File::open(ack).unwrap();
                // perf's FIFO acknowledgement includes its trailing NUL.
                let mut reply = [0u8; 5];
                reader.read_exact(&mut reply).unwrap();
                assert_eq!(&reply, b"ack\n\0");
            }
        };
        perf("enable\n");
        for _ in 0..count {
            assert_eq!(black_box(iter()), expected, "{}: checksum differs between iterations", name);
        }
        perf("disable\n");
        println!("| {} fixed_iters={} {}", name, count, expected);
        return;
    }
    let mut starts: Vec<f64> = Vec::new(); // ms since main start, at each iteration's start
    let mut times: Vec<f64> = Vec::new();
    let mut checksum = 0u64;
    // RW_O3_PROGRESS=<file> (Relax side; the JIT's MAKEPAD_RUN_O3_PROGRESS file `pending installs`): the
    // timed window starts at the first of QUIET_ITERS consecutive iterations that ran with no O3 build
    // queued or running and no install since the previous one; the run goes on until 10 iterations
    // after it (at most RW_MAX_MS). Without the file (rustc) the window starts RW_STEADY_AFTER_MS in.
    const QUIET_ITERS: usize = 5;
    let progress = std::env::var("RW_O3_PROGRESS").ok();
    let max_ms: f64 = std::env::var("RW_MAX_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(120000.0);
    let read_progress = |path: &str| -> Option<(u64, u64)> {
        let t = std::fs::read_to_string(path).ok()?;
        let mut it = t.split_whitespace();
        Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
    };
    let mut quiet = 0usize;
    let mut last_installs = u64::MAX;
    let mut steady_from: Option<usize> = None;
    loop {
        let s = Instant::now();
        starts.push(s.duration_since(t0).as_secs_f64() * 1000.0);
        let c = black_box(iter());
        times.push(s.elapsed().as_secs_f64() * 1000.0);
        if times.len() == 1 {
            checksum = c;
        } else {
            assert_eq!(c, checksum, "{}: checksum differs between iterations", name);
        }
        if let (Some(path), None) = (&progress, steady_from) {
            match read_progress(path) {
                Some((0, installs)) if installs == last_installs => {
                    quiet += 1;
                    if quiet >= QUIET_ITERS {
                        steady_from = Some(times.len() - QUIET_ITERS);
                    }
                }
                Some((_, installs)) => {
                    quiet = 0;
                    last_installs = installs;
                }
                None => quiet = 0,
            }
        }
        let elapsed = begin.elapsed().as_secs_f64() * 1000.0;
        if times.len() >= min_iters && elapsed >= budget_ms {
            match (&progress, steady_from) {
                (None, _) => break,
                (Some(_), Some(f)) if times.len() >= f + 10 => break,
                _ if elapsed >= max_ms => break,
                _ => {}
            }
        }
    }
    let n = times.len();
    // steady = the median of the last iterations that start after RW_STEADY_AFTER_MS (default 5000)
    // from main: Relax installs its optimised code about 2 s in, so earlier iterations are not steady
    let after: f64 = std::env::var("RW_STEADY_AFTER_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(5000.0);
    let eligible = match steady_from {
        Some(f) => n - f,
        None => starts.iter().filter(|s| **s >= after).count(),
    };
    let last = eligible.min(10).max(3).min(n);
    let steady = median(&times[n - last..]);
    // o3_ms: when the optimised code settled (ms since main; -1: no progress file, 0 in the field
    // `o3=` means it never settled within RW_MAX_MS and the old rule was used)
    let o3_ms = match (&progress, steady_from) {
        (None, _) => -1.0,
        (Some(_), Some(f)) => starts[f],
        (Some(_), None) => 0.0,
    };
    // settled: first i whose 3-iteration median is within 10% of steady
    let mut settled = n - 1;
    for i in 0..n {
        let w = &times[i..(i + 3).min(n)];
        if (median(w) - steady).abs() <= 0.10 * steady {
            settled = i;
            break;
        }
    }
    let tts_main = starts[settled];
    let tts_proc = match proc_ms_at_start {
        Some(p) => p + tts_main,
        None => tts_main,
    };
    println!("| {} steady_ms={:.3} tts_main_ms={:.1} tts_proc_ms={:.1} iters={} first_ms={:.3} o3_ms={:.0} {}", name, steady, tts_main, tts_proc, n, times[0], o3_ms, checksum);
}

pub fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
