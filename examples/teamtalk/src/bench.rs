//! Headless loopback benchmark for makepad-teamtalk: no audio devices, no
//! LAN port. Two links on ephemeral ports talk over 127.0.0.1; a simulated
//! capture clock pushes 48 kHz blocks on one side, a simulated device pulls
//! them on the other, and a click train measures the transport + jitter
//! latency (what the crate ADDS between the input callback and the output
//! callback — device block latency and DAC/ADC latency come on top).
//!
//!     cargo run -p makepad-example-teamtalk --bin teamtalk-bench --release -- \
//!         [--frame=240] [--block=240] [--secs=10]
//!
//! `--block` is the simulated device callback size at 48 kHz.

use makepad_teamtalk::{VoiceConfig, VoiceLink, INTERNAL_RATE};
use std::time::{Duration, Instant};

/// Wait until `due` with sub-100 µs accuracy: sleep the bulk, spin the rest.
/// (`thread::sleep` alone overshoots by 1-3 ms on macOS, which makes the
/// adaptive jitter buffer grow to cover the BENCH's own clock jitter; real
/// audio callbacks are hardware-paced.)
fn pace(due: Instant) {
    // TEAMTALK_BENCH_SLEEP=1: plain sleep pacing — latency numbers become
    // meaningless (the sleep jitter dominates) but process CPU then shows
    // the actual voice-path cost instead of the spin loop.
    if std::env::var_os("TEAMTALK_BENCH_SLEEP").is_some() {
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        }
        return;
    }
    loop {
        let now = Instant::now();
        if now >= due {
            return;
        }
        let left = due - now;
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(2));
        } else {
            std::hint::spin_loop();
        }
    }
}

struct Args {
    frame: usize,
    block: usize,
    secs: u64,
    ogg: bool,
}

fn main() {
    let mut args = Args {
        frame: 240,
        block: 240,
        secs: 10,
        ogg: false,
    };
    for a in std::env::args().skip(1) {
        if let Some(v) = a.strip_prefix("--frame=") {
            args.frame = v.parse().unwrap_or(240);
        } else if let Some(v) = a.strip_prefix("--block=") {
            args.block = v.parse().unwrap_or(240);
        } else if let Some(v) = a.strip_prefix("--secs=") {
            args.secs = v.parse().unwrap_or(10);
        } else if a == "--ogg" {
            args.ogg = true;
        }
    }

    let base = VoiceConfig {
        port: 0,
        broadcast: false,
        hello_ms: 250,
        // Gate off: the bench measures the continuous-audio path (worst
        // case for bandwidth, and clicks must not be eaten by the gate).
        gate_threshold_rms: -1.0,
        frame_samples: args.frame,
        codec: if args.ogg {
            makepad_teamtalk::Codec::Ogg
        } else {
            makepad_teamtalk::Codec::RawI16
        },
        ..VoiceConfig::default()
    };
    let mut tx = VoiceLink::bind(base.clone()).expect("bind tx");
    let tx_addr: std::net::SocketAddr = format!("127.0.0.1:{}", tx.local_addr().port())
        .parse()
        .unwrap();
    let mut rx = VoiceLink::bind(VoiceConfig {
        static_peers: vec![tx_addr],
        ..base
    })
    .expect("bind rx");

    // Wait for mutual discovery.
    let deadline = Instant::now() + Duration::from_secs(5);
    while tx.peers().is_empty() || rx.peers().is_empty() {
        assert!(Instant::now() < deadline, "discovery timed out");
        std::thread::sleep(Duration::from_millis(10));
    }

    let mut capture = tx.take_capture().unwrap();
    let mut playback = rx.take_playback().unwrap();
    let block = args.block;
    let block_dur = Duration::from_secs_f64(block as f64 / INTERNAL_RATE);
    let secs = args.secs;

    // Sender thread: a device-like clock pushing `block` samples every
    // block period. Every 500 ms the first sample is a 0.9 click; the click
    // push times go to the channel.
    let (click_tx, click_rx) = std::sync::mpsc::channel::<Instant>();
    let sender = std::thread::spawn(move || {
        let start = Instant::now();
        let mut input = vec![0.0f32; block];
        let mut n = 0u64;
        let clicks_every = (0.5 * INTERNAL_RATE / block as f64) as u64;
        loop {
            pace(start + block_dur * n as u32);
            if start.elapsed() > Duration::from_secs(secs) {
                break;
            }
            input.fill(0.0);
            if n % clicks_every.max(1) == 10 {
                input[0] = 0.9;
                let _ = click_tx.send(Instant::now());
            }
            capture.push_mono(INTERNAL_RATE, &input);
            n += 1;
        }
        capture.frames_sent()
    });

    // Receiver: the simulated output device, same block clock.
    let start = Instant::now();
    let mut out = vec![0.0f32; block];
    let mut latencies_us: Vec<u64> = Vec::new();
    let mut pending: Vec<Instant> = Vec::new();
    let mut n = 0u64;
    loop {
        pace(start + block_dur * n as u32);
        if start.elapsed() > Duration::from_secs(secs) + Duration::from_millis(300) {
            break;
        }
        let pull_start = Instant::now();
        out.fill(0.0);
        playback.mix_into_mono(INTERNAL_RATE, &mut out);
        while let Ok(t) = click_rx.try_recv() {
            pending.push(t);
        }
        if let Some(offset) = out.iter().position(|v| v.abs() > 0.4) {
            if let Some(pos) = pending.iter().position(|t| *t <= pull_start) {
                let t_click = pending.remove(pos);
                let t_play =
                    pull_start + Duration::from_secs_f64(offset as f64 / INTERNAL_RATE);
                latencies_us.push(t_play.duration_since(t_click).as_micros() as u64);
            }
        }
        n += 1;
    }
    let frames_sent = sender.join().unwrap();

    let stats = rx.stats();
    let peer = &rx.peers()[0];
    latencies_us.sort_unstable();
    let pct = |p: f64| {
        latencies_us
            .get(((latencies_us.len() as f64 - 1.0) * p) as usize)
            .copied()
            .unwrap_or(0)
    };
    println!("codec {}, frame {} samples ({:.2} ms), simulated device block {} samples ({:.2} ms), {} s",
        if args.ogg { "ogg (4-bit adpcm)" } else { "raw_i16" },
        args.frame, args.frame as f64 / 48.0, block, block as f64 / 48.0, args.secs);
    println!(
        "clicks measured {}: added latency min {:.2} ms  median {:.2} ms  p90 {:.2} ms  max {:.2} ms",
        latencies_us.len(),
        pct(0.0) as f64 / 1000.0,
        pct(0.5) as f64 / 1000.0,
        pct(0.9) as f64 / 1000.0,
        pct(1.0) as f64 / 1000.0,
    );
    println!(
        "sender: {} frames -> receiver: {} packets, {} bytes ({:.1} kbit/s), late {}, dup {}, accepted {}",
        frames_sent,
        stats.packets_recv,
        stats.bytes_recv,
        stats.bytes_recv as f64 * 8.0 / 1000.0 / args.secs as f64,
        peer.frames_late,
        peer.frames_duplicate,
        peer.frames_accepted,
    );
    println!(
        "receiver jitter buffer: target {} frames, buffered {:.1} ms",
        peer.target_frames, peer.buffered_ms
    );
}
