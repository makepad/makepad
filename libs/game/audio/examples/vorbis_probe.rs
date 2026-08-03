//! Diagnostic: compare our Vorbis decode against a reference WAV.
//! Usage: vorbis_probe <file.ogg> <reference.wav>
use makepad_game_audio as audio;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ogg = std::fs::read(&args[1]).expect("read ogg");
    let refwav = std::fs::read(&args[2]).expect("read ref");
    let got = audio::decode(&ogg).expect("decode ogg");
    let want = audio::wav::decode(&refwav).expect("decode ref");
    println!(
        "got {}ch {}Hz {} frames | ref {}ch {}Hz {} frames",
        got.channels,
        got.sample_rate,
        got.frames(),
        want.channels,
        want.sample_rate,
        want.frames()
    );

    let g = &got.samples;
    let w = &want.samples;
    let rms = |v: &[f32]| (v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    println!("rms got={:.6e} ref={:.6e} ratio(ref/got)={:.3}", rms(g), rms(w), rms(w) / rms(g));
    println!(
        "peak got={:.6e} ref={:.6e}",
        g.iter().fold(0f32, |a, b| a.max(b.abs())),
        w.iter().fold(0f32, |a, b| a.max(b.abs()))
    );

    // Best correlation over a lag sweep, plus the scale that best fits there.
    let n = g.len().min(w.len());
    let mut best = (0i64, 0f64, 0f64);
    for lag in -2000i64..2000 {
        let (mut num, mut dg, mut dw) = (0f64, 0f64, 0f64);
        for i in 0..n {
            let j = i as i64 + lag;
            if j < 0 || j as usize >= n {
                continue;
            }
            let (a, b) = (g[i] as f64, w[j as usize] as f64);
            num += a * b;
            dg += a * a;
            dw += b * b;
        }
        if dg > 0.0 && dw > 0.0 {
            let c = num / (dg.sqrt() * dw.sqrt());
            if c.abs() > best.1.abs() {
                best = (lag, c, num / dg.max(1e-30));
            }
        }
    }
    println!("best corr {:.4} at lag {} (fit scale ref=got*{:.4})", best.1, best.0, best.2);

    for &l in &[-1088i64,-1024,-901,-773,-640,-576,-512,-256,-128,0,128] {
        let (mut num, mut dg, mut dw) = (0f64,0f64,0f64);
        for i in 0..n { let j=i as i64+l; if j<0||j as usize>=n {continue;}
            let (a,b)=(g[i] as f64, w[j as usize] as f64); num+=a*b; dg+=a*a; dw+=b*b; }
        if dg>0.0&&dw>0.0 { println!("  lag {:>6}: corr {:.4}", l, num/(dg.sqrt()*dw.sqrt())); }
    }
    let on = |v: &[f32], thr: f32| v.iter().position(|s| s.abs() > thr);
    let (pg, pw) = (g.iter().fold(0f32,|a,b| a.max(b.abs())), w.iter().fold(0f32,|a,b| a.max(b.abs())));
    println!("onset(1% of peak) got={:?} ref={:?}", on(g, pg*0.01), on(w, pw*0.01));
    println!("first 6 got: {:?}", &g[..6.min(g.len())]);
    println!("first 6 ref: {:?}", &w[..6.min(w.len())]);
    if g.len() > 780 { println!("got[773..779]: {:?}", &g[773..779]); }
    println!("tail 4 got: {:?}", &g[g.len().saturating_sub(4)..]);
    println!("tail 4 ref: {:?}", &w[w.len().saturating_sub(4)..]);

    // Per-window scale, to expose block-size dependence.
    let win = 512;
    let mut ratios = Vec::new();
    for k in 0..(n / win) {
        let (a, b) = (&g[k * win..(k + 1) * win], &w[k * win..(k + 1) * win]);
        let (ra, rb) = (rms(a), rms(b));
        if ra > 1e-9 && rb > 1e-9 {
            ratios.push((k, rb / ra));
        }
    }
    let show: Vec<String> = ratios.iter().take(14).map(|(k, r)| format!("{k}:{r:.1}")).collect();
    println!("per-{win} ratios(ref/got): {}", show.join(" "));
    if !ratios.is_empty() {
        let mut v: Vec<f64> = ratios.iter().map(|(_, r)| *r).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!("ratio median={:.2} min={:.2} max={:.2}", v[v.len() / 2], v[0], v[v.len() - 1]);
    }
}

#[allow(dead_code)]
fn onset(v: &[f32], thr: f32) -> Option<usize> { v.iter().position(|s| s.abs() > thr) }
