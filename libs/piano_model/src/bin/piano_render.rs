//! piano_render <out.f32> [seconds]: renders the full-load script (88 keys struck every 1.5 s,
//! pedal down; 256-sample blocks at 48 kHz) and writes the stereo samples as little-endian f32
//! (left, then right). A bit-for-bit reference to compare builds of the model (rustc vs Relax,
//! kernels, targets); prints the render's real-time load.

use makepad_piano_model::{Piano, PianoEvent, TimedEvent};
use std::time::Instant;

const FS: f32 = 48000.0;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(out) = args.get(1) else {
        eprintln!("usage: piano_render <out.f32> [seconds]");
        std::process::exit(2);
    };
    let seconds: f64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
    // (sample, event): pedal down, then every key struck every 1.5 s
    let mut script: Vec<(u64, PianoEvent)> = vec![(0, PianoEvent::Sustain { value: 1.0 })];
    let mut t = 0.0;
    while t < seconds - 0.1 {
        for key in 21..=108u8 {
            let at = t + (key as f64 - 21.0) * 0.0001;
            script.push(((at * FS as f64).round() as u64, PianoEvent::NoteOn { key, velocity: 110 }));
        }
        t += 1.5;
    }
    let total = (seconds * FS as f64) as usize;
    let mut piano = Piano::new(FS);
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut events = Vec::new();
    let start = Instant::now();
    let mut pos = 0;
    while pos < total {
        let n = 256.min(total - pos);
        events.clear();
        for &(at, event) in &script {
            if at >= pos as u64 && at < (pos + n) as u64 {
                events.push(TimedEvent { offset: (at - pos as u64) as u32, event });
            }
        }
        piano.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    let took = start.elapsed().as_secs_f64();
    println!("piano_render: {:.1}% of real time ({:.3} s for {} s)", took / seconds * 100.0, took, seconds);
    let mut bytes = Vec::with_capacity(total * 8);
    for x in l.iter().chain(r.iter()) {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(out, bytes).expect("write the samples");
}
