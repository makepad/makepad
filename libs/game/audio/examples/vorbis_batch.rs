//! Batch-validate the Vorbis decoder against afconvert references.
//! Usage: vorbis_batch <dir-with-ogg> <tmpdir> [limit]
//!
//! Reports the correlation distribution so a systematic decoder fault is
//! visible as a cluster, not hidden behind one lucky or unlucky file.
use makepad_game_audio as audio;
use std::process::Command;

fn corr(a: &[f32], b: &[f32]) -> (f64, f64) {
    let n = a.len().min(b.len());
    let (mut num, mut da, mut db) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let (x, y) = (a[i] as f64, b[i] as f64);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        return (0.0, 0.0);
    }
    (num / (da.sqrt() * db.sqrt()), num / da)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let tmp = &args[2];
    let limit: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60);

    let mut files: Vec<_> = walk(dir);
    files.sort();
    let mut done = 0usize;
    let mut worst: Vec<(f64, String, usize, usize, u16)> = Vec::new();
    let (mut ok, mut framebad, mut failed) = (0usize, 0usize, 0usize);

    for f in files.iter() {
        if done >= limit {
            break;
        }
        let refwav = format!("{tmp}/ref_batch.wav");
        let _ = std::fs::remove_file(&refwav);
        let st = Command::new("afconvert")
            .args(["-f", "WAVE", "-d", "LEF32", f, &refwav])
            .status();
        if !matches!(st, Ok(s) if s.success()) {
            continue;
        }
        let (Ok(o), Ok(w)) = (std::fs::read(f), std::fs::read(&refwav)) else { continue };
        done += 1;
        let got = match audio::decode(&o) {
            Ok(p) => p,
            Err(e) => {
                failed += 1;
                worst.push((-1.0, format!("{f}  DECODE ERR {e:?}"), 0, 0, 0));
                continue;
            }
        };
        let want = match audio::wav::decode(&w) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let (c, _scale) = corr(&got.samples, &want.samples);
        // afconvert trims a further half-window off the tail, so our output
        // being exactly 128 frames longer is correct, not a mismatch.
        let d = got.frames() as i64 - want.frames() as i64;
        if d != 0 && d != 128 {
            framebad += 1;
        }
        if c > 0.999 && (d == 0 || d == 128) {
            ok += 1;
        }
        worst.push((c, f.clone(), got.frames(), want.frames(), got.channels as u16));
    }

    worst.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("=== {done} files: {ok} exact(>0.999 & frames match), {framebad} frame-count mismatch, {failed} decode errors");
    println!("--- 12 worst:");
    for (c, f, gf, wf, ch) in worst.iter().take(12) {
        let name = f.rsplit('/').next().unwrap_or(f);
        println!("  corr {c:.4}  {ch}ch  frames {gf}/{wf}  {name}");
    }
    let mono: Vec<f64> = worst.iter().filter(|x| x.4 == 1 && x.0 >= 0.0).map(|x| x.0).collect();
    let ster: Vec<f64> = worst.iter().filter(|x| x.4 == 2 && x.0 >= 0.0).map(|x| x.0).collect();
    let mean = |v: &[f64]| if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
    let lo = |v: &[f64]| v.iter().cloned().fold(f64::INFINITY, f64::min);
    println!("--- mono n={} mean {:.5} min {:.5}", mono.len(), mean(&mono), lo(&mono));
    println!("--- stereo n={} mean {:.5} min {:.5}", ster.len(), mean(&ster), lo(&ster));
}

fn walk(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p.to_string_lossy()));
        } else if p.extension().map(|x| x == "ogg").unwrap_or(false) {
            out.push(p.to_string_lossy().into_owned());
        }
    }
    out
}
