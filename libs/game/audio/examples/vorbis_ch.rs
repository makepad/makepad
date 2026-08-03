//! Per-channel comparison: tells swapped channels apart from a broken
//! coupling (one channel right, the other wrong).
//! Usage: vorbis_ch <file.ogg> <ref.wav>
use makepad_game_audio as audio;

fn corr(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    let (mut num, mut da, mut db) = (0f64, 0f64, 0f64);
    for i in 0..n {
        let (x, y) = (a[i] as f64, b[i] as f64);
        num += x * y;
        da += x * x;
        db += y * y;
    }
    if da <= 0.0 || db <= 0.0 {
        return 0.0;
    }
    num / (da.sqrt() * db.sqrt())
}

fn chan(p: &audio::Pcm, c: usize) -> Vec<f32> {
    p.samples.iter().skip(c).step_by(p.channels).cloned().collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let got = audio::decode(&std::fs::read(&a[1]).expect("ogg")).expect("decode");
    let want = audio::wav::decode(&std::fs::read(&a[2]).expect("ref")).expect("ref");
    println!("ch={} got={} ref={}", got.channels, got.frames(), want.frames());
    if got.channels < 2 {
        println!("mono: corr {:.4}", corr(&got.samples, &want.samples));
        return;
    }
    let (gl, gr) = (chan(&got, 0), chan(&got, 1));
    let (wl, wr) = (chan(&want, 0), chan(&want, 1));
    println!("  L->L {:.4}   L->R {:.4}", corr(&gl, &wl), corr(&gl, &wr));
    println!("  R->R {:.4}   R->L {:.4}", corr(&gr, &wr), corr(&gr, &wl));
    // Mid/side view: coupling errors usually leave the mid intact and wreck
    // the side, which is invisible in a plain per-channel correlation.
    let mid = |l: &[f32], r: &[f32]| -> Vec<f32> {
        l.iter().zip(r).map(|(a, b)| (a + b) * 0.5).collect()
    };
    let side = |l: &[f32], r: &[f32]| -> Vec<f32> {
        l.iter().zip(r).map(|(a, b)| (a - b) * 0.5).collect()
    };
    println!(
        "  mid {:.4}   side {:.4}",
        corr(&mid(&gl, &gr), &mid(&wl, &wr)),
        corr(&side(&gl, &gr), &side(&wl, &wr))
    );
}
