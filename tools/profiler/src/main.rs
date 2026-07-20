mod flamegraph;
mod macho;
mod trace_file;

use flamegraph::{write_flamegraph, FlamegraphOptions};
use makepad_micro_serde::SerJson;
use makepad_profiler::{capture, capture_while, CaptureCollector, CaptureConfig, ProfilerError};
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use trace_file::TraceFile;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("makepad-profiler: {}", err);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), ProfilerError> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "capture" => run_capture(&args[1..]),
        "demo-target" => run_demo_target(&args[1..]),
        "record" | "--record" => run_record(&args),
        "help" | "-h" | "--help" => {
            print_help();
            Ok(())
        }
        trace_path if trace_path.ends_with(".ptrace") || Path::new(trace_path).exists() => {
            run_trace_command(&args)
        }
        command => Err(ProfilerError::new(format!(
            "unknown command `{}`; run `makepad-profiler help` for usage",
            command
        ))),
    }
}

fn run_capture(args: &[String]) -> Result<(), ProfilerError> {
    let mut process_id = None;
    let mut output = None;
    let mut duration_ms = 1_000u64;
    let mut interval_us = 1_000u64;
    let mut max_frames = 128usize;
    let mut include_images = true;

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--pid" => {
                process_id = Some(parse_value(args, &mut index, "--pid")?.parse::<u32>().map_err(
                    |err| ProfilerError::new(format!("invalid --pid value: {}", err)),
                )?);
            }
            "--output" => {
                output = Some(PathBuf::from(parse_value(args, &mut index, "--output")?));
            }
            "--duration-ms" => {
                duration_ms =
                    parse_value(args, &mut index, "--duration-ms")?
                        .parse::<u64>()
                        .map_err(|err| {
                            ProfilerError::new(format!("invalid --duration-ms value: {}", err))
                        })?;
            }
            "--interval-us" => {
                interval_us =
                    parse_value(args, &mut index, "--interval-us")?
                        .parse::<u64>()
                        .map_err(|err| {
                            ProfilerError::new(format!("invalid --interval-us value: {}", err))
                        })?;
            }
            "--max-frames" => {
                max_frames =
                    parse_value(args, &mut index, "--max-frames")?
                        .parse::<usize>()
                        .map_err(|err| {
                            ProfilerError::new(format!("invalid --max-frames value: {}", err))
                        })?;
            }
            "--no-images" => {
                include_images = false;
                index += 1;
            }
            unexpected => {
                return Err(ProfilerError::new(format!(
                    "unexpected capture argument `{}`",
                    unexpected
                )));
            }
        }
    }

    let process_id = process_id.ok_or_else(|| ProfilerError::new("missing required --pid"))?;
    let output = output.unwrap_or_else(|| PathBuf::from(format!("profile-{}.json", process_id)));
    let config = CaptureConfig {
        process_id,
        duration: Duration::from_millis(duration_ms),
        interval: Duration::from_micros(interval_us),
        max_frames,
        include_images,
    };
    let capture = capture(&config)?;
    std::fs::write(&output, capture.serialize_json()).map_err(|err| {
        ProfilerError::new(format!(
            "failed to write capture file {}: {}",
            output.display(),
            err
        ))
    })?;

    println!(
        "wrote {} samples across {} thread snapshots to {}",
        capture.sample_count(),
        capture.thread_sample_count(),
        output.display()
    );
    if !capture.warnings.is_empty() {
        for warning in &capture.warnings {
            eprintln!("warning: {}", warning);
        }
    }
    Ok(())
}

fn run_record(args: &[String]) -> Result<(), ProfilerError> {
    let mut index = 1usize;
    let record_mode = if index < args.len() && !args[index].starts_with("--") {
        let mode = args[index].clone();
        index += 1;
        mode
    } else {
        "cpuperf".to_string()
    };
    if record_mode != "cpuperf" {
        return Err(ProfilerError::new(format!(
            "unsupported record mode `{}`; only `cpuperf` is implemented right now",
            record_mode
        )));
    }

    let mut output = None;
    let mut interval_us = 1_000u64;
    let mut max_frames = 128usize;
    let mut include_images = true;
    let mut program = None;
    let mut program_args = Vec::new();

    while index < args.len() {
        match args[index].as_str() {
            "--run" => {
                program = Some(parse_value(args, &mut index, "--run")?.to_string());
            }
            "--output" => {
                output = Some(PathBuf::from(parse_value(args, &mut index, "--output")?));
            }
            "--interval-us" => {
                interval_us = parse_value(args, &mut index, "--interval-us")?
                    .parse::<u64>()
                    .map_err(|err| {
                        ProfilerError::new(format!("invalid --interval-us value: {}", err))
                    })?;
            }
            "--max-frames" => {
                max_frames = parse_value(args, &mut index, "--max-frames")?
                    .parse::<usize>()
                    .map_err(|err| {
                        ProfilerError::new(format!("invalid --max-frames value: {}", err))
                    })?;
            }
            "--no-images" => {
                include_images = false;
                index += 1;
            }
            "--" => {
                program_args.extend(args[index + 1..].iter().cloned());
                break;
            }
            unexpected => {
                return Err(ProfilerError::new(format!(
                    "unexpected record argument `{}`",
                    unexpected
                )));
            }
        }
    }

    let program = program.ok_or_else(|| ProfilerError::new("missing required --run"))?;
    let output = output.unwrap_or_else(|| default_trace_path(&program));
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            ProfilerError::new(format!(
                "failed to create trace directory {}: {}",
                parent.display(),
                err
            ))
        })?;
    }

    let mut command = std::process::Command::new(&program);
    command
        .args(&program_args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = command.spawn().map_err(|err| {
        ProfilerError::new(format!("failed to launch `{}`: {}", program, err))
    })?;

    // Let the child exec and settle before we resolve its task port and
    // capture the first loaded-image snapshot.
    std::thread::sleep(Duration::from_millis(25));

    let started = Instant::now();
    let mut collector = CaptureCollector::default();
    let mut exit_status = None;
    let config = CaptureConfig {
        process_id: child.id(),
        duration: Duration::from_secs(1),
        interval: Duration::from_micros(interval_us),
        max_frames,
        include_images,
    };

    capture_while(&config, &mut collector, || {
        if let Some(status) = child.try_wait().map_err(|err| {
            ProfilerError::new(format!("failed to poll child process status: {}", err))
        })? {
            exit_status = Some(status);
            return Ok(false);
        }
        Ok(true)
    })?;

    if exit_status.is_none() {
        exit_status = Some(child.wait().map_err(|err| {
            ProfilerError::new(format!("failed to wait for child process: {}", err))
        })?);
    }

    let mut capture = collector.finish()?;
    capture.header.duration_micros = started.elapsed().as_micros() as u64;
    let trace = TraceFile::from_capture(capture);
    trace.write_to_path(&output)?;

    println!("trace captured: {}", output.display());
    println!("symbols resolved: {}", trace.symbols.len());
    if let Some(status) = exit_status {
        println!("process exited with status {}", status);
    }
    Ok(())
}

fn run_trace_command(args: &[String]) -> Result<(), ProfilerError> {
    if args.len() < 2 {
        return Err(ProfilerError::new(
            "expected a trace command after the .ptrace path",
        ));
    }

    let trace = TraceFile::read_from_path(Path::new(&args[0]))?;
    match args[1].as_str() {
        "functions" => run_functions_report(&trace, &args[2..]),
        "flamegraph" => run_flamegraph_report(&trace, Path::new(&args[0]), &args[2..]),
        command => Err(ProfilerError::new(format!(
            "unknown trace subcommand `{}`",
            command
        ))),
    }
}

fn run_functions_report(trace: &TraceFile, args: &[String]) -> Result<(), ProfilerError> {
    let mut limit = 20usize;
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "-n" | "--limit" => {
                limit = parse_value(args, &mut index, "-n/--limit")?
                    .parse::<usize>()
                    .map_err(|err| ProfilerError::new(format!("invalid limit: {}", err)))?;
            }
            unexpected => {
                return Err(ProfilerError::new(format!(
                    "unexpected functions argument `{}`",
                    unexpected
                )));
            }
        }
    }

    let rows = trace.function_rows(limit);
    println!(
        "{:<32} {:>10} {:>6} {:>12} {:>6} {:>13}",
        "Function", "Excl (us)", "(%)", "Incl (us)", "(%)", "Blocked (us)"
    );
    for row in rows {
        println!(
            "{:<32} {:>10.1} {:>6.1} {:>12.1} {:>6.1} {:>13.1}",
            clip_name(&row.name, 32),
            row.exclusive_micros,
            row.exclusive_percent,
            row.inclusive_micros,
            row.inclusive_percent,
            row.blocked_micros
        );
    }
    Ok(())
}

fn run_flamegraph_report(
    trace: &TraceFile,
    trace_path: &Path,
    args: &[String],
) -> Result<(), ProfilerError> {
    let mut output = trace_path.with_extension("svg");
    let mut title = trace
        .header
        .executable
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("makepad-profiler")
        .to_string();
    let mut include_blocked = false;
    let mut width = 1600usize;

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                output = PathBuf::from(parse_value(args, &mut index, "--output")?);
            }
            "--title" => {
                title = parse_value(args, &mut index, "--title")?.to_string();
            }
            "--include-blocked" => {
                include_blocked = true;
                index += 1;
            }
            "--width" => {
                width = parse_value(args, &mut index, "--width")?
                    .parse::<usize>()
                    .map_err(|err| ProfilerError::new(format!("invalid width: {}", err)))?;
            }
            unexpected => {
                return Err(ProfilerError::new(format!(
                    "unexpected flamegraph argument `{}`",
                    unexpected
                )));
            }
        }
    }

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            ProfilerError::new(format!(
                "failed to create flamegraph directory {}: {}",
                parent.display(),
                err
            ))
        })?;
    }

    write_flamegraph(
        trace,
        &output,
        &FlamegraphOptions {
            title,
            include_blocked,
            width,
        },
    )?;
    println!("flamegraph written: {}", output.display());
    Ok(())
}

fn run_demo_target(args: &[String]) -> Result<(), ProfilerError> {
    let mut threads = 4usize;
    let mut depth = 48u32;
    let mut seconds = None;
    let mut index = 0usize;

    while index < args.len() {
        match args[index].as_str() {
            "--threads" => {
                threads = parse_value(args, &mut index, "--threads")?
                    .parse::<usize>()
                    .map_err(|err| ProfilerError::new(format!("invalid --threads value: {}", err)))?;
            }
            "--depth" => {
                depth = parse_value(args, &mut index, "--depth")?
                    .parse::<u32>()
                    .map_err(|err| ProfilerError::new(format!("invalid --depth value: {}", err)))?;
            }
            "--seconds" => {
                seconds = Some(
                    parse_value(args, &mut index, "--seconds")?
                        .parse::<u64>()
                        .map_err(|err| {
                            ProfilerError::new(format!("invalid --seconds value: {}", err))
                        })?,
                );
            }
            unexpected => {
                return Err(ProfilerError::new(format!(
                    "unexpected demo-target argument `{}`",
                    unexpected
                )));
            }
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    println!("{}", std::process::id());

    let mut workers = Vec::new();
    for worker_index in 0..threads {
        let stop = Arc::clone(&stop);
        workers.push(std::thread::spawn(move || busy_worker(worker_index as u64 + 1, depth, stop)));
    }

    if let Some(seconds) = seconds {
        std::thread::sleep(Duration::from_secs(seconds));
        stop.store(true, Ordering::Relaxed);
    } else {
        while !stop.load(Ordering::Relaxed) {
            std::thread::park_timeout(Duration::from_secs(1));
        }
    }

    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn parse_value<'a>(
    args: &'a [String],
    index: &mut usize,
    flag: &str,
) -> Result<&'a str, ProfilerError> {
    if *index + 1 >= args.len() {
        return Err(ProfilerError::new(format!("missing value for {}", flag)));
    }
    let value = &args[*index + 1];
    *index += 2;
    Ok(value)
}

fn default_trace_path(program: &str) -> PathBuf {
    let stem = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("trace");
    PathBuf::from("traces").join(format!("{}.ptrace", stem))
}

fn clip_name(name: &str, width: usize) -> String {
    if name.chars().count() <= width {
        return name.to_string();
    }
    let clipped: String = name.chars().take(width.saturating_sub(3)).collect();
    format!("{}...", clipped)
}

#[inline(never)]
fn busy_worker(mut seed: u64, depth: u32, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        seed = black_box(busy_frame(seed, depth));
        if seed & 0x1fff == 0 {
            std::thread::yield_now();
        }
    }
}

#[inline(never)]
fn busy_frame(seed: u64, depth: u32) -> u64 {
    if depth == 0 {
        return busy_leaf(seed);
    }
    let next = seed
        .rotate_left((depth & 31) + 1)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(depth as u64);
    busy_frame(next, depth - 1).wrapping_add(seed ^ next)
}

#[inline(never)]
fn busy_leaf(seed: u64) -> u64 {
    let mut value = seed;
    for _ in 0..1024 {
        value = value
            .rotate_left(7)
            .wrapping_mul(0xD6E8_FEB8_6659_FD93)
            .wrapping_add(0xA076_1D64_78BD_642F);
        black_box(value);
    }
    value
}

fn print_help() {
    println!(
        "makepad-profiler\n\
         \n\
         Commands:\n\
           record [cpuperf] --run <program> [--output traces/app.ptrace] [--interval-us 1000] [--max-frames 128] [--no-images] [-- child-args...]\n\
           <trace>.ptrace functions [-n 20]\n\
           <trace>.ptrace flamegraph [--output traces/app.svg] [--title App] [--width 1600] [--include-blocked]\n\
           capture --pid <pid> [--output profile.json] [--duration-ms 1000] [--interval-us 1000] [--max-frames 128] [--no-images]\n\
           demo-target [--threads 4] [--depth 48] [--seconds 30]\n\
         \n\
         Notes:\n\
           - macOS only for now\n\
           - record currently implements CPU sampling via frame-pointer unwinding\n\
           - builds with frame pointers produce the best stacks\n\
           - self-profiling is intentionally rejected; profile a separate child process instead"
    );
}
