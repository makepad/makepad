//! program-bench: compute programs on nodes, as a person steering them feels it.
//!
//! ```text
//! program-bench [--nodes host:port,...] [--tiles N] [--res R] [--iter I] [--load N]
//!               [--class interactive,visible,background]
//! ```
//!
//! Steers the fractal program (`makepad_compute_program::programs::FRACTAL`) on each node a
//! tile at a time, the way a zoom does: the time from a tile's message to its texels here is
//! one frame's latency (p50, p95, max). Each class is measured on the node as it is, then next
//! to a build started here: `--load` threads (default: one per core; 0: none) spinning in a
//! scope of their own, weighted as a build of that many cores is (Linux, under the user's
//! service manager). Every tile is bit-compared with the same program run in-process.
//!
//! Nodes: `--nodes` (fleet `host:port`, or `http://` loopback), else a node started in this
//! process on loopback (the build then runs on its cores, as on a busy node).

use makepad_ai_hub::kernel_client::{KernelNode, KernelStream};
use makepad_ai_hub::program_client::{RemoteConfig, RemoteSession};
use makepad_compute_program::programs::FRACTAL;
use makepad_compute_program::{Class, Limits, OutputData, Program, Session, SessionConfig, Value};
use std::sync::Arc;
use std::time::Instant;

struct Opts {
    nodes: Option<String>,
    tiles: usize,
    res: usize,
    iter: u32,
    load: usize,
    classes: Vec<Class>,
}

fn die(m: &str) -> ! {
    eprintln!("program-bench: {m}");
    std::process::exit(2)
}

fn main() {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut o = Opts { nodes: None, tiles: 40, res: 256, iter: 500, load: cores, classes: vec![Class::Interactive, Class::Background] };
    let mut args = std::env::args().skip(1);
    if std::env::args().nth(1).as_deref() == Some(BURN) {
        burn(std::env::args().nth(2).and_then(|n| n.parse().ok()).unwrap_or(cores));
    }
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| die(&format!("{a} needs a value")));
        match a.as_str() {
            "--nodes" => o.nodes = Some(val()),
            "--tiles" => o.tiles = val().parse().unwrap_or_else(|_| die("--tiles N")),
            "--res" => o.res = val().parse().unwrap_or_else(|_| die("--res R")),
            "--iter" => o.iter = val().parse().unwrap_or_else(|_| die("--iter I")),
            "--load" => o.load = val().parse().unwrap_or_else(|_| die("--load N")),
            "--class" => {
                o.classes = val()
                    .split(',')
                    .map(|c| match c.trim() {
                        "interactive" => Class::Interactive,
                        "visible" => Class::Visible,
                        "background" => Class::Background,
                        _ => die("--class interactive|visible|background,..."),
                    })
                    .collect()
            }
            _ => die(&format!("unknown argument {a}")),
        }
    }
    let urls: Vec<String> = match &o.nodes {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|h| {
                if h.starts_with("http://") {
                    h.to_string()
                } else {
                    makepad_network::fleet::id::mark_fleet_endpoint(h);
                    format!("https://{h}")
                }
            })
            .collect(),
        None => vec![loopback_node()],
    };
    let views = views(o.tiles, o.res);
    let reference = in_process(&views, o.res, o.iter);
    println!("program-bench: {} tiles of {}x{}, {} iterations, {} cores here", o.tiles, o.res, o.res, o.iter, cores);
    for class in &o.classes {
        println!("{:<12} {}", format!("{class:?}").to_lowercase(), os_class(*class));
    }
    println!("{:<28} {:<12} {:>9} {:>9} {:>9} {:>8} {:>7}", "node", "class", "p50 ms", "p95 ms", "max ms", "tiles/s", "weight");
    for url in &urls {
        let node = KernelNode::new(url);
        let stream = match KernelStream::open(&node) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                println!("{url}: {e:?}");
                continue;
            }
        };
        let name = url.trim_start_matches("https://").trim_start_matches("http://");
        for loaded in [false, true] {
            if loaded && o.load == 0 {
                continue;
            }
            let label = if loaded { format!("build of {}", o.load) } else { "no build".to_string() };
            let build = match loaded.then(|| Build::start(o.load)) {
                Some(Err(e)) => {
                    println!("{name} {label}: {e}");
                    continue;
                }
                build => build,
            };
            for class in &o.classes {
                match steer(&stream, *class, &views, o.res, o.iter, &reference) {
                    Ok((mut ms, weight)) => {
                        let total: f64 = ms.iter().sum();
                        ms.sort_by(f64::total_cmp);
                        let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
                        println!(
                            "{:<28} {:<12} {:>9.1} {:>9.1} {:>9.1} {:>8.1} {:>7}",
                            format!("{name} {label}"),
                            format!("{class:?}").to_lowercase(),
                            at(0.5),
                            at(0.95),
                            at(1.0),
                            ms.len() as f64 / (total / 1000.0),
                            weight
                        );
                    }
                    Err(e) => println!("{name} {label} {class:?}: {e}"),
                }
            }
            drop(build);
        }
    }
}

/// `tiles` views along a zoom into the seahorse valley, a tile of `res` each.
fn views(tiles: usize, res: usize) -> Vec<[f64; 3]> {
    (0..tiles)
        .map(|k| {
            let width = 0.05 * 0.93f64.powi(k as i32);
            [-0.743_643_9 - 0.5 * width, 0.131_825_9 - 0.5 * width, width / res as f64]
        })
        .collect()
}

fn tile(slot: usize, at: [f64; 3], res: usize, iter: u32) -> Value {
    let n = Value::Number;
    Value::object(&[
        ("slot", n(slot as f64)),
        ("x", n(at[0])),
        ("y", n(at[1])),
        ("step", n(at[2])),
        ("res", n(res as f64)),
        ("kind", n(0.0)),
        ("power", n(2.0)),
        ("max_iter", n(iter as f64)),
        ("de", n(1.0)),
        ("trap", n(0.0)),
        ("julia", Value::Array(vec![n(0.0), n(0.0)])),
    ])
}

fn words(o: &makepad_compute_program::Output) -> Vec<u32> {
    match &o.data {
        OutputData::Words(w) => w.to_vec(),
        _ => Vec::new(),
    }
}

/// The tiles in-process: what every node must send back.
fn in_process(views: &[[f64; 3]], res: usize, iter: u32) -> Vec<Vec<u32>> {
    let session = Session::open(&Program::new("fractal.splash", FRACTAL), SessionConfig::default());
    let out = views
        .iter()
        .map(|at| {
            let rev = session.send("tile", tile(0, *at, res, iter));
            loop {
                let u = session.wait();
                if let Some(Err(e)) = &u.end {
                    die(&format!("in-process: {e}"));
                }
                if let Some(o) = u.output("tile0").filter(|o| o.rev == rev) {
                    break words(o);
                }
            }
        })
        .collect();
    session.cancel();
    out
}

/// What the OS gives a thread of `class`: whether the class applied, its nice and policy.
fn os_class(class: Class) -> String {
    std::thread::spawn(move || {
        let applied = class.apply();
        #[cfg(target_os = "linux")]
        {
            extern "C" {
                fn getpriority(which: i32, who: u32) -> i32;
                fn sched_getscheduler(pid: i32) -> i32;
            }
            let (nice, policy) = unsafe { (getpriority(0, 0), sched_getscheduler(0)) };
            let policy = match policy {
                0 => "other",
                3 => "batch",
                5 => "idle",
                _ => "?",
            };
            format!("applied {applied}, nice {nice}, {policy}")
        }
        #[cfg(not(target_os = "linux"))]
        format!("applied {applied}")
    })
    .join()
    .unwrap_or_default()
}

/// Each view's latency on the node in ms, a tile at a time (a zoom that waits for its frame),
/// and the node's CPU weight while the session ran ("-": none it can set).
fn steer(stream: &Arc<KernelStream>, class: Class, views: &[[f64; 3]], res: usize, iter: u32, reference: &[Vec<u32>]) -> Result<(Vec<f64>, String), String> {
    let program = Program::new("fractal.splash", FRACTAL);
    let limits = Limits { fuel: usize::MAX, work: u64::MAX, ..Limits::default() };
    let session = RemoteSession::open(stream, &program, RemoteConfig { limits, class, ..RemoteConfig::default() }).map_err(|e| format!("{e:?}"))?;
    let weight = match session.device().get("weight") {
        Some(Value::Number(w)) => format!("{w}"),
        _ => "-".to_string(),
    };
    // the kernels compile on the first tile: not a steering latency
    let warm = session.send("tile", tile(1, views[0], res, iter));
    take(&session, "tile1", warm)?;
    let mut ms = Vec::with_capacity(views.len());
    for (k, at) in views.iter().enumerate() {
        let start = Instant::now();
        let rev = session.send("tile", tile(0, *at, res, iter));
        let texels = take(&session, "tile0", rev)?;
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
        if texels != reference[k] {
            return Err(format!("tile {k} differs from the in-process tile"));
        }
    }
    session.cancel();
    Ok((ms, weight))
}

fn take(session: &RemoteSession, name: &str, rev: u64) -> Result<Vec<u32>, String> {
    loop {
        let u = session.wait();
        if let Some(o) = u.output(name).filter(|o| o.rev == rev) {
            return Ok(words(o));
        }
        if let Some(end) = u.end {
            return Err(format!("the program ended: {end:?}"));
        }
    }
}

/// A node on loopback in this process.
fn loopback_node() -> String {
    use makepad_ai_hub::server::{start_service, ServiceConfig};
    let dir = std::env::temp_dir().join(format!("makepad-program-bench-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let handle = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: dir,
        registry: makepad_ai_hub::registry::Registry::embedded().unwrap_or_else(|e| die(&format!("registry: {e:?}"))),
        downloader: makepad_ai_hub::download::Downloader::new("http://127.0.0.1:1", None).unwrap_or_else(|e| die(&format!("downloader: {e:?}"))),
        peer: makepad_ai_hub::peer_serve::PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
        fleet: makepad_ai_hub::discovery::DEFAULT_FLEET.to_string(),
    })
    .unwrap_or_else(|e| die(&format!("loopback node: {e:?}")));
    let url = format!("http://{}", handle.addr);
    // the node lives as long as the bench
    std::mem::forget(handle);
    url
}

/// The bench run as a build: `program-bench --burn N`.
const BURN: &str = "--burn";

/// A build next to the node: `n` threads spinning in a scope of their own, weighted as devhub
/// weighs a lease of `n` cores (100 a core).
struct Build {
    child: std::process::Child,
}

impl Build {
    fn start(n: usize) -> Result<Build, String> {
        use std::io::BufRead;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut child = std::process::Command::new("systemd-run")
            .args(["--user", "--scope", "--quiet", "--collect", "-p", &format!("CPUWeight={}", (100 * n).min(10_000))])
            .arg(exe)
            .args([BURN, &n.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("a build needs systemd-run (a Linux user manager): {e}"))?;
        // `burning` once its threads spin
        let mut line = String::new();
        let read = std::io::BufReader::new(child.stdout.take().ok_or("no build output")?).read_line(&mut line);
        if read.is_err() || line.trim() != "burning" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("the build did not start: {}", line.trim()));
        }
        Ok(Build { child })
    }
}

impl Drop for Build {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `n` threads spinning until this process is killed.
fn burn(n: usize) -> ! {
    for _ in 0..n {
        std::thread::spawn(|| {
            let mut x = 1u64;
            loop {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        });
    }
    println!("burning");
    loop {
        std::thread::park();
    }
}
