//! Derive the trim rule: search (start, len) against a reference so the
//! correct offset is measured, not assumed.
//! Usage: vorbis_trim <file.ogg> <ref.wav>
use makepad_game_audio as audio;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let ogg = std::fs::read(&a[1]).expect("ogg");
    let refw = std::fs::read(&a[2]).expect("ref");
    let (buf, first_center, granule) = audio::vorbis::debug_raw(&ogg).expect("raw");
    let want = audio::wav::decode(&refw).expect("ref wav");
    let ch = buf.len();
    let buflen = buf[0].len();
    let wf = want.frames();
    println!("buflen={buflen} first_center={first_center} granule={granule} ref_frames={wf} ch={ch}");

    // Exact-match search: for each candidate start, how well does
    // buf[start .. start+ref_frames] line up with the reference?
    let mut best = (0usize, f64::INFINITY);
    let lo = first_center.saturating_sub(1024);
    let hi = (first_center + 1024).min(buflen.saturating_sub(wf));
    for start in lo..=hi {
        if start + wf > buflen {
            break;
        }
        let mut err = 0f64;
        // Sparse but dense enough to rank candidates unambiguously.
        let mut i = 0usize;
        while i < wf {
            for c in 0..ch {
                let g = buf[c][start + i] as f64;
                let w = want.samples[i * ch + c] as f64;
                err += (g - w) * (g - w);
            }
            i += 7;
        }
        if err < best.1 {
            best = (start, err);
        }
    }
    let rms = (best.1 / (wf / 7).max(1) as f64).sqrt();
    println!(
        "BEST start={} (first_center{:+}) rms_err={:.3e}  | granule-reflen={}",
        best.0,
        best.0 as i64 - first_center as i64,
        rms,
        granule as i64 - wf as i64
    );
    println!(
        "  tail: buflen-(start+reflen)={}",
        buflen as i64 - (best.0 + wf) as i64
    );
    // Is the region our decode emits before the reference's first sample
    // actually silent? If so afconvert trimmed it and we are not wrong.
    let rms_of = |lo: usize, hi: usize| -> f64 {
        if hi <= lo {
            return 0.0;
        }
        let mut s = 0f64;
        let mut n = 0usize;
        for c in 0..ch {
            for i in lo..hi.min(buflen) {
                s += (buf[c][i] as f64).powi(2);
                n += 1;
            }
        }
        if n == 0 { 0.0 } else { (s / n as f64).sqrt() }
    };
    let peak_of = |lo: usize, hi: usize| -> f64 {
        let mut p = 0f64;
        for c in 0..ch {
            for i in lo..hi.min(buflen) {
                p = p.max((buf[c][i] as f64).abs());
            }
        }
        p
    };
    println!(
        "  lead region [{}..{}]: rms={:.3e} peak={:.3e}   (signal rms={:.3e})",
        first_center,
        best.0,
        rms_of(first_center, best.0),
        peak_of(first_center, best.0),
        rms_of(best.0, best.0 + wf.min(4096))
    );
}
