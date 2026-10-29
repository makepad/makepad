//! A debugging aid: `TANDEM_PROBE=1` prints filter magnitude responses for a
//! few types at low cutoffs (skipped without the variable).

use makepad_audio_synth::kit::flt::FilterUnit;

fn gain(ty: u8, sub: u8, cut: f32, reso: f32, hz: f32) -> f32 {
    let fs = 96000.0;
    let mut f = FilterUnit::default();
    let n = 96000;
    let mut peak = 0.0f32;
    for i in 0..n {
        if i % 64 == 0 {
            f.set_block(ty, sub, cut, reso, fs, true);
        }
        let x = 0.01 * (std::f32::consts::TAU * hz * i as f32 / fs).sin();
        let y = f.tick(x);
        if i > n / 2 {
            peak = peak.max(y.abs());
        }
    }
    20.0 * (peak / 0.01).log10()
}

#[test]
fn probe_filters() {
    if std::env::var_os("TANDEM_PROBE").is_none() {
        return;
    }
    for (ty, sub) in [(4u8, 0u8), (4, 1), (4, 2), (1, 1)] {
        let line: Vec<String> = [16.0f32, 33.0, 49.0, 66.0, 99.0, 200.0, 1000.0].iter().map(|hz| format!("{hz}:{:.1}", gain(ty, sub, -37.9, 0.574, *hz))).collect();
        eprintln!("type {ty} sub {sub}: {}", line.join(" "));
    }
}
