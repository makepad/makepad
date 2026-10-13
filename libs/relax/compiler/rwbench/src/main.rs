// Real-world benchmark workloads for Relax (phase 1): the same source under rustc --release and
// `makepad run`. Each workload is `setup() -> iteration closure`; setup (reading inputs) is not
// timed per iteration but counts towards time-to-steady. Usage: relax-rwbench <workload>...
//
// Output per workload (one line):
//   | <name> steady_ms=<f> tts_main_ms=<f> tts_proc_ms=<f> iters=<n> first_ms=<f> <checksum>
// steady_ms   median of the last N iteration times (N = min(10, iters/4), at least 3)
// tts_main_ms ms from entering main until the iteration the per-iteration time settles within 10%
//             of steady_ms (3-iteration running median) starts; tts_proc_ms the same measured from
//             the process start (RW_WALL0 from run.py, else /proc on Linux; includes Relax's compile
//             before main; else equal to tts_main_ms)
// Env: RW_MS (run each workload at least this many ms, default 3000), RW_MIN_ITERS (default 20),
//      RW_ROOT (the makepad tree root holding the inputs; default: the tree this crate is in).
//      RW_ITERS (positive count: warm up for RW_MS, then run exactly this many iterations),
//      RW_PERF_CTL=<control fifo>,<ack fifo> (fixed mode only; perf stat --control fifo:...
//      -D -1 --no-inherit counts the main thread only, excluding setup and warm-up).
use std::hint::black_box;
use std::time::Instant;

mod inputs;
mod layout;
mod llm;
mod maps;
mod parsers;
mod sdf;
mod text;

fn main() {
    let t0 = Instant::now();
    let proc_ms_at_start = process_age_ms();
    let only: Vec<String> = std::env::args().skip(1).collect();
    let all: [(&str, fn() -> Box<dyn FnMut() -> u64>); 12] = [
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
    let budget_ms: f64 = std::env::var("RW_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000.0);
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
        if times.len() >= min_iters && begin.elapsed().as_secs_f64() * 1000.0 >= budget_ms {
            break;
        }
    }
    let n = times.len();
    let last = (n / 4).min(10).max(3).min(n);
    let steady = median(&times[n - last..]);
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
    println!("| {} steady_ms={:.3} tts_main_ms={:.1} tts_proc_ms={:.1} iters={} first_ms={:.3} {}", name, steady, tts_main, tts_proc, n, times[0], checksum);
}

pub fn fold(h: u64, v: u64) -> u64 {
    (h.rotate_left(5) ^ v).wrapping_mul(0x9E37_79B9_7F4A_7C15)
}
