//! Where does the reference's opening actually live in our raw buffer?
//!
//! `vorbis_exact` compares the TRIMMED output, so a wrong head and a wrong
//! trim look alike. This searches the untrimmed overlap-add buffer for the
//! reference's first frames. If they appear exactly somewhere, the decode is
//! fine and only `first_center` is wrong; if they appear nowhere, the early
//! packets genuinely decode wrong and the trim is innocent.
//!
//! Usage: vorbis_head <file.ogg> <reference.wav> [probe_frames]
use makepad_game_audio as audio;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let ogg = std::fs::read(&args[1]).expect("read ogg");
    let refwav = std::fs::read(&args[2]).expect("read ref");
    let probe: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(512);

    let (raw, _ch, granule) = audio::vorbis::debug_raw(&ogg).expect("raw");
    let sizes = audio::vorbis::debug_block_sizes(&ogg).expect("sizes");
    let want = audio::wav::decode(&refwav).expect("ref");
    let wch = want.channels as usize;
    println!(
        "raw {} chans x {} samples, granule {granule}, first block sizes {:?}",
        raw.len(),
        raw[0].len(),
        &sizes[..sizes.len().min(12)]
    );

    // Reference channel 0 only: coupling is symmetric here and one channel is
    // enough to locate the head.
    let w0: Vec<f32> = (0..want.frames().min(probe)).map(|f| want.samples[f * wch]).collect();
    let q = |x: f32| (x * 32768.0).round() as i32;

    let g0 = &raw[0];
    let mut best = (0usize, 0usize, f64::INFINITY);
    for off in 0..g0.len().saturating_sub(w0.len()) {
        let mut exact = 0usize;
        let mut sad = 0f64;
        for i in 0..w0.len() {
            if q(g0[off + i]) == q(w0[i]) {
                exact += 1;
            }
            sad += (g0[off + i] - w0[i]).abs() as f64;
        }
        let m = sad / w0.len() as f64;
        if exact > best.1 || (exact == best.1 && m < best.2) {
            best = (off, exact, m);
        }
    }
    println!(
        "reference head best match at raw offset {} : {}/{} exact ({:.1}%), mean|diff| {:.6}",
        best.0,
        best.1,
        w0.len(),
        100.0 * best.1 as f64 / w0.len() as f64,
        best.2
    );
    if best.1 * 100 / w0.len().max(1) >= 99 {
        println!("VERDICT: head decodes correctly — first_center should be {}", best.0);
    } else {
        println!("VERDICT: the head is nowhere in the raw buffer — early packets decode wrong");
    }
}
