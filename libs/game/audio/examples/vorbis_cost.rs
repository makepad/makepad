//! Decode cost and memory across a pack, for the Quest budget.
use makepad_game_audio as audio;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let files = walk(&a[1]);
    let (mut n, mut bin, mut pcm, mut fr) = (0usize, 0usize, 0usize, 0usize);
    let t0 = Instant::now();
    let mut worst = (0f64, String::new());
    for f in &files {
        let Ok(b) = std::fs::read(f) else { continue };
        let t = Instant::now();
        let Ok(p) = audio::decode(&b) else { continue };
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms > worst.0 { worst = (ms, f.rsplit('/').next().unwrap_or(f).to_string()); }
        n += 1; bin += b.len(); pcm += p.samples.len() * 4; fr += p.frames();
    }
    let total = t0.elapsed().as_secs_f64() * 1000.0;
    println!("decoded {n} files in {total:.0} ms ({:.2} ms/file avg)", total / n.max(1) as f64);
    println!("slowest {:.2} ms  {}", worst.0, worst.1);
    println!("compressed {:.1} MB -> f32 PCM {:.1} MB ({fr} frames)",
        bin as f64 / 1048576.0, pcm as f64 / 1048576.0);
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
