//! Is a mismatch a TRIM bug or a CONTENT bug?
//!
//! Correlation cannot tell those apart: a transient decoded correctly but
//! offset scores badly, and a transient decoded wrongly can still score 0.8 on
//! envelope shape alone. This sweeps whole-frame lags looking for an EXACT
//! sample match instead. A lag where the overlap matches bit-for-bit means the
//! decode is right and only the trim is wrong; no such lag anywhere means the
//! samples themselves differ and the trim is a red herring.
//!
//! Usage: vorbis_exact <file.ogg> <reference.wav> [max_lag_frames]
use makepad_game_audio as audio;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ogg = std::fs::read(&args[1]).expect("read ogg");
    let refwav = std::fs::read(&args[2]).expect("read ref");
    let max_lag: i64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2048);

    let got = audio::decode(&ogg).expect("decode ogg");
    let want = audio::wav::decode(&refwav).expect("decode ref");
    let ch = got.channels as usize;
    println!(
        "got {}ch {} frames | ref {}ch {} frames | delta {}",
        got.channels,
        got.frames(),
        want.channels,
        want.frames(),
        got.frames() as i64 - want.frames() as i64
    );

    let g = &got.samples;
    let w = &want.samples;

    // afconvert writes 16-bit, so our f32 can only ever match to quantisation.
    // Compare at that resolution or every sample "differs" for no useful reason.
    let q = |x: f32| (x * 32768.0).round() as i32;

    let mut best = (0i64, 0usize, 0usize, f64::INFINITY);
    for lag in -max_lag..=max_lag {
        let off = lag * ch as i64;
        let (gs, ws) = if off >= 0 {
            (off as usize, 0usize)
        } else {
            (0usize, (-off) as usize)
        };
        if gs >= g.len() || ws >= w.len() {
            continue;
        }
        let n = (g.len() - gs).min(w.len() - ws);
        if n < ch * 256 {
            continue;
        }
        let mut exact = 0usize;
        let mut sad = 0f64;
        for i in 0..n {
            let (a, b) = (g[gs + i], w[ws + i]);
            if q(a) == q(b) {
                exact += 1;
            }
            sad += (a - b).abs() as f64;
        }
        let mean_abs = sad / n as f64;
        if exact > best.1 || (exact == best.1 && mean_abs < best.3) {
            best = (lag, exact, n, mean_abs);
        }
    }
    let (lag, exact, n, mean_abs) = best;
    println!(
        "best lag {lag} frames: {exact}/{n} samples exact ({:.2}%), mean|diff| {mean_abs:.6}",
        100.0 * exact as f64 / n as f64
    );
    if exact * 100 / n.max(1) >= 99 {
        println!("VERDICT: decode is correct, trim is off by {lag} frames");
    } else {
        println!("VERDICT: content differs — trim alone cannot explain this");
    }
}
