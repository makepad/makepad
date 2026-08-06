//! Correlate block geometry against the true start offset, so the lapping
//! rule is derived from data instead of guessed.
//! Usage: vorbis_align <dir> <tmp> [limit]
use makepad_game_audio as audio;
use std::process::Command;

fn best_lag(g: &[f32], w: &[f32], ch: usize) -> (i64, f64) {
    let n = g.len().min(w.len());
    let mut best = (0i64, 0f64);
    // Lags are whole frames; step by channel count so stereo isn't tested
    // at half-frame offsets that can never be right.
    let step = ch as i64;
    let mut lag = -8192i64;
    while lag <= 8192 {
        let (mut num, mut dg, mut dw) = (0f64, 0f64, 0f64);
        let mut i = 0usize;
        while i < n {
            let j = i as i64 + lag;
            if j >= 0 && (j as usize) < n {
                let (a, b) = (g[i] as f64, w[j as usize] as f64);
                num += a * b;
                dg += a * a;
                dw += b * b;
            }
            i += 16; // sparse sample: enough to find the peak, 16x faster
        }
        if dg > 0.0 && dw > 0.0 {
            let c = num / (dg.sqrt() * dw.sqrt());
            if c > best.1 {
                best = (lag, c);
            }
        }
        lag += step;
    }
    best
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (dir, tmp) = (&a[1], &a[2]);
    let limit: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut files = walk(dir);
    files.sort();
    println!("{:>4} {:>5} {:>5} {:>8} {:>8} {:>7} {:>7}  file", "ch", "n0", "n1", "granule", "reflen", "lagfrm", "corr");
    let mut done = 0;
    for f in files.iter() {
        if done >= limit { break }
        let refwav = format!("{tmp}/align_ref.wav");
        let _ = std::fs::remove_file(&refwav);
        if !matches!(Command::new("afconvert").args(["-f","WAVE","-d","LEF32",f,&refwav]).status(), Ok(s) if s.success()) { continue }
        let (Ok(o), Ok(wv)) = (std::fs::read(f), std::fs::read(&refwav)) else { continue };
        let Ok(got) = audio::decode(&o) else { continue };
        let Ok(want) = audio::wav::decode(&wv) else { continue };
        done += 1;
        let (lag, c) = best_lag(&got.samples, &want.samples, got.channels);
        let bs = audio::debug_block_sizes(&o).unwrap_or_default();
        let name = f.rsplit('/').next().unwrap_or(f);
        println!("{:>4} {:>5} {:>5} {:>8} {:>8} {:>7} {:>7.4}  {}",
            got.channels,
            bs.first().copied().unwrap_or(0),
            bs.get(1).copied().unwrap_or(0),
            got.frames(),
            want.frames(),
            -lag / got.channels as i64,
            c, name);
    }
}

fn walk(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() { out.extend(walk(&p.to_string_lossy())) }
        else if p.extension().map(|x| x == "ogg").unwrap_or(false) { out.push(p.to_string_lossy().into_owned()) }
    }
    out
}
