//! List audio clients, or play one application's audio through this process.
//!
//! `route list` prints clients and output devices.
//! `route play <bundle-id> [--seconds N] [--gain G]` taps that application,
//! silences its direct output while the tap is read, and plays it here.
//! The host bundle needs NSAudioCaptureUsageDescription; the first play
//! asks for system audio recording permission.

use makepad_audio_route::{Equalizer, FrameInfo, Gain, Mute, Passthrough, Processor, Route, RouteConfig, Source};
use std::env;
use std::sync::atomic::{AtomicU32, Ordering};
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
    eprintln!("       route play <bundle-id> [--seconds N] [--gain G] [--eq]");
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
            _ => index += 1,
        }
    }
    println!("routing {bundle} for {seconds}s at gain {gain}");
    let peak_bits = Arc::new(AtomicU32::new(0f32.to_bits()));
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
    let route = open(&bundle, Box::new(Meter { inner, peak_bits: Arc::clone(&peak_bits) }))?;
    println!("open: {:.0} Hz, {} ch", route.sample_rate(), route.channels());
    thread::sleep(Duration::from_secs(seconds));
    drop(route);
    println!("peak {:0.4}", f32::from_bits(peak_bits.load(Ordering::Relaxed)));
    Ok(())
}

struct Meter {
    inner: Box<dyn Processor>,
    peak_bits: Arc<AtomicU32>,
}

impl Processor for Meter {
    fn process(&mut self, frames: &mut [f32], info: &FrameInfo) {
        self.inner.process(frames, info);
        let mut peak = f32::from_bits(self.peak_bits.load(Ordering::Relaxed));
        for sample in frames.iter() {
            peak = peak.max(sample.abs());
        }
        self.peak_bits.store(peak.to_bits(), Ordering::Relaxed);
    }
}

fn open(bundle: &str, processor: Box<dyn makepad_audio_route::Processor>) -> Result<Route, makepad_audio_route::Error> {
    Route::open(
        RouteConfig {
            sources: vec![Source::BundleId(bundle.to_string())],
            mute: Mute::ReplaceWhileRouted,
            output: None,
        },
        processor,
    )
}
