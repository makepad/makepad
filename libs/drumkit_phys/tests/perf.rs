// CPU cost (ignored by default; run explicitly, in release):
//   cargo test -p makepad-drumkit-phys --release --test perf -- --ignored --nocapture
// Renders a dense full-kit pattern (hats on 16ths, kick/snare/clap, crash
// and ride ringing, tom fill: 8-14 voices sounding at any time) and reports
// the share of one core at 48 kHz. The budget the crate promises is
// "well under 5 %"; the assertion holds it under 5 % with the ignored test
// so a slow CI box does not fail the default suite.

use makepad_drumkit_phys::{DrumKit, DrumVoice};
use std::time::Instant;

fn dense_pattern(seconds: f32, fs: f32) -> Vec<(usize, DrumVoice, f32)> {
    let spb = 0.5 * fs; // 120 bpm
    let mut hits = Vec::new();
    let bars = (seconds / 2.0) as usize;
    for bar in 0..bars {
        let b0 = (bar as f32 * 4.0 * spb) as usize;
        for i in 0..16 {
            hits.push((b0 + (i as f32 * 0.25 * spb) as usize, if i % 4 == 2 { DrumVoice::HiHatOpen } else { DrumVoice::HiHatClosed }, 0.8));
        }
        for beat in [0.0f32, 1.75, 2.5] {
            hits.push((b0 + (beat * spb) as usize, DrumVoice::Kick, 1.0));
        }
        for beat in [1.0f32, 3.0] {
            hits.push((b0 + (beat * spb) as usize, DrumVoice::Snare, 1.0));
            hits.push((b0 + (beat * spb) as usize, DrumVoice::Clap, 0.9));
        }
        for i in 0..8 {
            hits.push((b0 + (i as f32 * 0.5 * spb) as usize, DrumVoice::Ride, 0.7));
        }
        hits.push((b0, DrumVoice::Crash, 1.0));
        hits.push((b0 + (2.0 * spb) as usize, DrumVoice::RideBell, 0.9));
        for (k, v) in [DrumVoice::TomHigh, DrumVoice::TomMid, DrumVoice::TomLow, DrumVoice::TomFloor].iter().enumerate() {
            hits.push((b0 + ((3.0 + 0.25 * k as f32) * spb) as usize, *v, 0.9));
        }
        hits.push((b0 + (3.5 * spb) as usize, DrumVoice::SideStick, 0.8));
        hits.push((b0 + (3.75 * spb) as usize, DrumVoice::HiHatPedal, 0.7));
    }
    hits.sort_by_key(|h| h.0);
    hits
}

#[test]
#[ignore]
fn perf_full_kit_pattern() {
    let fs = 48000.0;
    let seconds = 20.0;
    let hits = dense_pattern(seconds, fs);
    let total = (seconds * fs) as usize;
    let block = 256;
    let mut kit = DrumKit::new(fs);
    let mut out = vec![[0.0f32; 2]; block];
    let mut next = 0;
    let start = Instant::now();
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        while next < hits.len() && hits[next].0 < pos + n {
            kit.trigger(hits[next].1, hits[next].2);
            next += 1;
        }
        out[..n].iter_mut().for_each(|f| *f = [0.0; 2]);
        kit.process(&mut out[..n]);
        pos += n;
    }
    let wall = start.elapsed().as_secs_f64();
    let share = 100.0 * wall / seconds as f64;
    println!("full kit pattern, {} hits over {seconds} s at 48 kHz, block {block}: {wall:.3} s wall = {share:.2} % of one core ({:.1}x realtime)", hits.len(), seconds as f64 / wall);
    assert!(share < 5.0, "CPU share {share:.2} % exceeds the 5 % budget");

    // construction cost (not real-time, but worth knowing)
    let t0 = Instant::now();
    let _k = DrumKit::new(fs);
    println!("DrumKit::new: {:.2} ms", t0.elapsed().as_secs_f64() * 1e3);
}
