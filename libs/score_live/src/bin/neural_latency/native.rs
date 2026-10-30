//! The native tool (see main.rs).

use makepad_audio_synth::Instrument;
use makepad_score_live::game::{music, Clocking};
use makepad_score_player::sung::neural;
use std::time::{Duration, Instant};

const RATE: f32 = 48_000.0;
const BPM: f64 = 100.0;
/// The loop's length in beats: the line, then a rest as long.
const BEATS: f64 = 8.0;

fn source(high: bool, words: &str) -> String {
    let n = if high { "@c5" } else { "@e4" };
    let engine = if std::env::args().any(|a| a == "--dsp") { "@dsp" } else { "@neural" };
    format!(
        "use mod.score.*\nlet clock = Clock{{bpm: {BPM}}}\nlet vox = Voice{{engine: {engine}}}\nlet line = Loop{{play: vox beats: {BEATS} code: fn(i) {{ sing([{n} {n} {n} {n}], \"{words}\", {{dur: 1}}) sleep({BEATS}) }}}}\nScore{{clock vox line}}\n"
    )
}

/// The pitch of 20 ms of audio (Hz), if it is voiced.
fn pitch(x: &[f32]) -> Option<f32> {
    let energy: f32 = x.iter().map(|v| v * v).sum();
    if energy < 1e-4 * x.len() as f32 {
        return None;
    }
    let (lo, hi) = ((RATE / 1000.0) as usize, (RATE / 150.0) as usize);
    let mut best = (0.0f32, 0usize);
    for lag in lo..hi.min(x.len() / 2) {
        let c: f32 = x[..x.len() - lag].iter().zip(&x[lag..]).map(|(a, b)| a * b).sum();
        if c > best.0 {
            best = (c, lag);
        }
    }
    (best.0 > 0.5 * energy).then(|| RATE / best.1 as f32)
}

pub fn run() {
    let edits: usize = std::env::args().skip(1).find_map(|v| v.parse().ok()).unwrap_or(4);
    let path = neural::model_path();
    let t = Instant::now();
    let Some(_singer) = neural::shared() else {
        eprintln!("no neural voice installed at {} (cantor_install)", path.display());
        std::process::exit(1);
    };
    println!("voice {} loaded in {:.0} ms", path.display(), t.elapsed().as_secs_f64() * 1e3);
    let lines = ["a-ma-zing grace", "how sweet the sound", "we are young", "bring me home"];
    let (mut handle, mut inst) = music(&source(false, lines[0]), RATE, Clocking::Realtime).unwrap_or_else(|d| panic!("{d:?}"));
    let chunk = 256usize;
    let first_edit = 7.0;
    // Edits at uneven places in the loop (4.8 s at 100 bpm).
    let gaps = [9.1, 10.3, 8.2, 11.7];
    let mut edit_times = Vec::new();
    let mut next_edit = first_edit;
    let total = first_edit + gaps.iter().take(edits).sum::<f64>() + 4.0;
    let frames = (total * RATE as f64) as usize;
    let mut out = Vec::with_capacity(frames);
    let start = Instant::now();
    let mut late_passes = 0usize;
    let (mut render_sum, mut render_max) = (0.0f64, 0.0f64);
    let mut high = false;
    while out.len() < frames {
        let at = out.len() as f64 / RATE as f64;
        if edit_times.len() < edits && at >= next_edit {
            high = !high;
            handle.set_source(&source(high, lines[(edit_times.len() + 1) % lines.len()])).unwrap_or_else(|d| panic!("{d:?}"));
            edit_times.push((at, high));
            next_edit += gaps[edit_times.len() - 1];
        }
        let (mut l, mut r) = (vec![0.0f32; chunk], vec![0.0f32; chunk]);
        let t = Instant::now();
        inst.render(&mut l, &mut r);
        let took = t.elapsed().as_secs_f64();
        render_sum += took;
        render_max = render_max.max(took);
        let _ = handle.poll();
        out.extend(l);
        // Real time: the next pass is due when this one has played.
        let due = start + Duration::from_secs_f64(out.len() as f64 / RATE as f64);
        let now = Instant::now();
        if now < due {
            std::thread::sleep(due - now);
        } else if now > due + Duration::from_millis(20) {
            late_passes += 1;
        }
    }
    // When each edited line is first heard: its pitch (C5 or E4) in the output.
    let win = (0.02 * RATE as f64) as usize;
    let mut heard = Vec::new();
    for (i, (at, high)) in edit_times.iter().enumerate() {
        let want = if *high { 523.25f32 } else { 329.63 };
        let end = edit_times.get(i + 1).map(|e| e.0).unwrap_or(total);
        let mut f = (*at * RATE as f64) as usize;
        let stop = ((end * RATE as f64) as usize).min(out.len() - win);
        let mut found = None;
        while f < stop {
            if let Some(p) = pitch(&out[f..f + win]) {
                if (1200.0 * (p / want).log2()).abs() < 80.0 {
                    found = Some(f as f64 / RATE as f64);
                    break;
                }
            }
            f += win / 2;
        }
        heard.push(found.map(|h| h - at));
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let passes = (out.len() / chunk).max(1);
    println!("played {total:.1} s in real time; passes more than 20 ms late: {late_passes}; a pass rendered in {:.3} ms on average, {:.1} ms at most (it plays {:.1} ms); peak {peak:.3}", render_sum / passes as f64 * 1e3, render_max * 1e3, chunk as f64 / RATE as f64 * 1e3);
    for ((at, high), h) in edit_times.iter().zip(&heard) {
        let loop_s = BEATS * 60.0 / BPM;
        let phase = (at % loop_s) / loop_s;
        match h {
            Some(h) => println!("edit at {at:.2} s ({} , {:.0}% into the loop): heard after {h:.2} s", if *high { "to C5" } else { "to E4" }, phase * 100.0),
            None => println!("edit at {at:.2} s: not heard"),
        }
    }
    let got: Vec<f64> = heard.iter().flatten().copied().collect();
    if !got.is_empty() {
        let mean = got.iter().sum::<f64>() / got.len() as f64;
        println!("edit to heard: mean {mean:.2} s, min {:.2} s, max {:.2} s (the line's next turn in the loop, {:.1} s long, decides most of it)", got.iter().cloned().fold(f64::MAX, f64::min), got.iter().cloned().fold(0.0, f64::max), BEATS * 60.0 / BPM);
    }
}
