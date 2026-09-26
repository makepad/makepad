//! List audio clients, or play one application's audio through this process.
//!
//! `route list` prints clients and output devices.
//! `route play <bundle-id> [--seconds N] [--gain G] [--eq] [--hear]
//! [--monitor] [--switch S] [--skip S]` taps that application, silences its
//! direct output while the tap is read (`--hear` leaves it playing), plays
//! it here, and prints the tapped and played levels once a second and their
//! average over the run after `--skip` seconds. `--monitor` starts
//! monitoring (the application plays by itself, nothing plays here);
//! `--switch` flips between monitoring and processing after S seconds.
//! The host bundle needs NSAudioCaptureUsageDescription; the first play
//! asks for system audio recording permission.

use makepad_audio_route::{Equalizer, FrameInfo, Gain, Mute, Passthrough, Processor, Route, RouteConfig, Source};
use std::env;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0] == "list" {
        list();
        return;
    }
    if args[0] == "play" {
        if let Err(error) = play(&args[1..]) {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    eprintln!("usage: route list");
    eprintln!("       route play <bundle-id> [--seconds N] [--gain G] [--eq] [--hear] [--monitor] [--switch S] [--skip S]");
    std::process::exit(2);
}

fn list() {
    match makepad_audio_route::processes() {
        Ok(processes) => {
            println!("clients");
            for process in processes {
                let state = if process.output_running { "playing" } else { "idle" };
                println!("  {state:7}  pid {:6}  {:<28}  {}", process.pid, process.bundle_id, process.name);
            }
        }
        Err(error) => eprintln!("clients: {error}"),
    }
    match makepad_audio_route::outputs() {
        Ok(devices) => {
            println!("outputs");
            for device in devices {
                println!("  {:5.0} Hz  {:2} ch  {}  {}", device.sample_rate, device.channels, device.uid, device.name);
            }
        }
        Err(error) => eprintln!("outputs: {error}"),
    }
}

fn play(args: &[String]) -> Result<(), makepad_audio_route::Error> {
    let bundle = args.first().filter(|arg| !arg.starts_with("--")).cloned().ok_or(makepad_audio_route::Error::NotFound)?;
    let mut seconds = 10u64;
    let mut gain = 1.0f32;
    let mut eq = false;
    let mut mute = Mute::ReplaceWhileRouted;
    let mut processing = true;
    let mut switch_at = 0u64;
    let mut skip = 0u64;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--seconds" => {
                seconds = args.get(index + 1).and_then(|value| value.parse().ok()).unwrap_or(seconds);
                index += 2;
            }
            "--gain" => {
                gain = args.get(index + 1).and_then(|value| value.parse().ok()).unwrap_or(gain);
                index += 2;
            }
            "--eq" => {
                eq = true;
                index += 1;
            }
            "--hear" => {
                mute = Mute::HearOriginal;
                index += 1;
            }
            "--monitor" => {
                processing = false;
                index += 1;
            }
            "--switch" => {
                switch_at = args.get(index + 1).and_then(|value| value.parse().ok()).unwrap_or(0);
                index += 2;
            }
            "--skip" => {
                skip = args.get(index + 1).and_then(|value| value.parse().ok()).unwrap_or(0);
                index += 2;
            }
            _ => index += 1,
        }
    }
    println!("routing {bundle} for {seconds}s at gain {gain}, {mute:?}, {}", if processing { "processing" } else { "monitoring" });
    let levels = Arc::new(Levels::default());
    let inner: Box<dyn Processor> = if eq {
        let (equalizer, knobs) = Equalizer::new();
        knobs.set_low_db(4.0);
        knobs.set_high_db(3.0);
        Box::new(equalizer)
    } else if (gain - 1.0).abs() > 0.001 {
        Box::new(Gain::new(gain).0)
    } else {
        Box::new(Passthrough)
    };
    let route = open(&bundle, mute, processing, Box::new(Meter { inner, levels: Arc::clone(&levels) }))?;
    println!("open: {:.0} Hz, {} ch", route.sample_rate(), route.channels());
    let (mut in_peak, mut out_peak) = (0f32, 0f32);
    let (mut energy, mut samples) = (0f64, 0f64);
    for second in 1..=seconds {
        thread::sleep(Duration::from_secs(1));
        let (rms, peak, played, count) = levels.take();
        in_peak = in_peak.max(peak);
        out_peak = out_peak.max(played);
        if second > skip {
            energy += (rms as f64).powi(2) * count as f64;
            samples += count as f64;
        }
        // The sample count shows whether frames arrive at all while the
        // application is silent (silent buffers) or not (no callbacks).
        println!("{second:3}s  tapped rms {:7.2} dBFS  peak {:7.2} dBFS  played peak {:7.2} dBFS  {count} samples", db(rms), db(peak), db(played));
        if second == switch_at {
            let _ = route.set_processing(!route.processing());
            println!("      switched to {}", if route.processing() { "processing" } else { "monitoring" });
        }
        if second % 2 == 0 || second == switch_at + 1 {
            println!("      {}", route.describe());
        }
    }
    drop(route);
    println!("peak tapped {in_peak:0.4}, played {out_peak:0.4}; tapped rms over the run after {skip}s: {:.3} dBFS", db((energy / samples.max(1.0)).sqrt() as f32));
    Ok(())
}

fn db(linear: f32) -> f32 {
    20.0 * linear.max(1e-10).log10()
}

/// What the tap delivered and what was played, since the last take.
#[derive(Default)]
struct Levels {
    /// The sum of squares, in units of 2^-40.
    sumsq: AtomicU64,
    count: AtomicU64,
    peak_in: AtomicU32,
    peak_out: AtomicU32,
}

impl Levels {
    /// RMS and peak of the tap, the played peak, and the samples metered;
    /// cleared.
    fn take(&self) -> (f32, f32, f32, u64) {
        let sum = self.sumsq.swap(0, Ordering::Relaxed) as f64 / (1u64 << 40) as f64;
        let count = self.count.swap(0, Ordering::Relaxed);
        let peak_in = f32::from_bits(self.peak_in.swap(0, Ordering::Relaxed));
        let peak_out = f32::from_bits(self.peak_out.swap(0, Ordering::Relaxed));
        ((sum / count.max(1) as f64).sqrt() as f32, peak_in, peak_out, count)
    }
}

struct Meter {
    inner: Box<dyn Processor>,
    levels: Arc<Levels>,
}

impl Processor for Meter {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        let (mut sum, mut peak) = (0f64, 0f32);
        for sample in frames.iter() {
            sum += (*sample as f64) * (*sample as f64);
            peak = peak.max(sample.abs());
        }
        self.levels.sumsq.fetch_add((sum * (1u64 << 40) as f64) as u64, Ordering::Relaxed);
        self.levels.count.fetch_add(frames.len() as u64, Ordering::Relaxed);
        self.levels.peak_in.fetch_max(peak.to_bits(), Ordering::Relaxed);
        self.inner.process(frames, info);
        let played = frames.iter().fold(0f32, |peak, sample| peak.max(sample.abs()));
        self.levels.peak_out.fetch_max(played.to_bits(), Ordering::Relaxed);
    }
}

fn open(bundle: &str, mute: Mute, processing: bool, processor: Box<dyn makepad_audio_route::Processor>) -> Result<Route, makepad_audio_route::Error> {
    Route::open(
        RouteConfig {
            sources: vec![Source::BundleId(bundle.to_string())],
            mute,
            output: None,
            processing,
            state_dir: Some(std::env::temp_dir()),
        },
        processor,
    )
}
