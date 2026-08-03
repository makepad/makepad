//! Which blocks are wrong, and what do they have in common?
//!
//! Once a file is known to decode mostly-exactly (see vorbis_exact), the
//! interesting question is the STRUCTURE of the failures: contiguous runs
//! aligned to block boundaries point at a per-packet decode fault, scattered
//! single samples point at rounding. This maps mismatches onto the block grid
//! and prints the first few bad regions with their neighbourhood, so the fault
//! can be tied to a specific packet.
//!
//! Usage: vorbis_badblocks <file.ogg> <reference.wav> [block] [show]
use makepad_game_audio as audio;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ogg = std::fs::read(&args[1]).expect("read ogg");
    let refwav = std::fs::read(&args[2]).expect("read ref");
    let block: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(128);
    let show: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(12);

    let got = audio::decode(&ogg).expect("decode ogg");
    let want = audio::wav::decode(&refwav).expect("decode ref");
    let ch = got.channels as usize;
    let g = &got.samples;
    let w = &want.samples;
    let q = |x: f32| (x * 32768.0).round() as i32;

    let frames = got.frames().min(want.frames());
    // Per-block agreement, plus per-channel so a coupling fault (one channel
    // right, the other wrong) is visible rather than averaged away.
    let nblocks = frames / block;
    let mut bad = Vec::new();
    for b in 0..nblocks {
        let mut exact = 0usize;
        let mut total = 0usize;
        let mut per_ch = vec![0usize; ch];
        for f in b * block..(b + 1) * block {
            for c in 0..ch {
                let i = f * ch + c;
                if i >= g.len() || i >= w.len() {
                    continue;
                }
                total += 1;
                if q(g[i]) == q(w[i]) {
                    exact += 1;
                    per_ch[c] += 1;
                }
            }
        }
        if total > 0 && exact * 100 / total < 95 {
            bad.push((b, exact, total, per_ch));
        }
    }
    println!(
        "{} frames, block {}: {}/{} blocks bad",
        frames,
        block,
        bad.len(),
        nblocks
    );
    for (b, exact, total, per_ch) in bad.iter().take(show) {
        let chs: Vec<String> = per_ch
            .iter()
            .map(|e| format!("{:.0}%", 100.0 * *e as f64 / (*total / ch).max(1) as f64))
            .collect();
        println!(
            "  block {b:5} frames {:6}..{:6}  exact {:.0}%  per-ch [{}]",
            b * block,
            (b + 1) * block,
            100.0 * *exact as f64 / *total as f64,
            chs.join(" ")
        );
    }
    if bad.is_empty() {
        return;
    }
    // First bad block, sample by sample: a constant ratio means amplitude,
    // a sign flip means polarity, noise means a decode divergence.
    let b0 = bad[0].0;
    println!("--- first bad block {b0}, first 8 frames:");
    for f in b0 * block..(b0 * block + 8).min(frames) {
        let mut row = format!("  f{f:6}");
        for c in 0..ch {
            let i = f * ch + c;
            let (a, e) = (g[i], w[i]);
            let ratio = if e.abs() > 1e-6 {
                format!("{:6.3}", a / e)
            } else {
                "   inf".to_string()
            };
            row.push_str(&format!("  ch{c} got {a:9.6} ref {e:9.6} r {ratio}"));
        }
        println!("{row}");
    }
}
