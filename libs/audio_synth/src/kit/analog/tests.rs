//! The components against their datasheets: each test is one published
//! figure, measured. `--nocapture` prints the table of measured values.

use super::blep::HALF;
use super::eg::{self, Eg, EgBlock, EgStage};
use super::vca::{self, Vca, VcaBlock};
use super::vcf::{self, Vcf};
use super::vco::{Edges, Vco, VcoBlock, VcoPart, NO_EDGES, PULSE_PEAK};
use std::f64::consts::TAU;

const RATE: f32 = 48_000.0;

/// In-place radix-2 FFT.
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -TAU / len as f64;
        for i in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f64).sin_cos();
                let (a, b) = (i + k, i + k + len / 2);
                let (tr, ti) = (re[b] * c - im[b] * s, re[b] * s + im[b] * c);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Magnitudes of a block (no window: tests use whole cycles).
fn spectrum(x: &[f32]) -> Vec<f64> {
    let mut re: Vec<f64> = x.iter().map(|v| *v as f64).collect();
    let mut im = vec![0.0; x.len()];
    fft(&mut re, &mut im);
    let scale = 2.0 / x.len() as f64;
    (0..x.len() / 2).map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt() * scale).collect()
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-12).log10()
}

/// What in a spectrum is not a harmonic of bin `k`: (the worst bin below
/// `audible` Hz in dB re the fundamental, all of it together in dB).
fn alias_of(mag: &[f64], k: usize, audible: f64) -> (f64, f64) {
    let n = mag.len() * 2;
    let hz = |i: usize| i as f64 * RATE as f64 / n as f64;
    let (mut worst, mut sum) = (0.0f64, 0.0f64);
    for (i, m) in mag.iter().enumerate().skip(1) {
        if i % k != 0 {
            sum += m * m;
            if hz(i) < audible {
                worst = worst.max(*m);
            }
        }
    }
    (db(worst / mag[k]), db(sum.sqrt() / mag[k]))
}

fn measure_hz(b: &VcoBlock, cycles: usize) -> f64 {
    let mut v = Vco::new();
    v.start(0.0);
    let (mut first, mut last, mut count) = (None, 0.0f64, 0usize);
    let mut i = 0u64;
    let limit = (cycles as f64 / b.inc as f64 * 2.0) as u64 + 100;
    while count <= cycles && i < limit {
        v.tick(b, 0.0, &NO_EDGES);
        if v.edges.wrap >= 0.0 {
            let t = i as f64 + v.edges.wrap as f64;
            first.get_or_insert(t);
            last = t;
            count += 1;
        }
        i += 1;
    }
    (count - 1) as f64 / (last - first.unwrap()) * RATE as f64
}

fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

#[test]
fn vco_tracks_one_volt_per_octave_over_the_sweep_range() {
    // 50,000:1: 0.4 Hz to 20 kHz.
    let mut hz = 0.4f32;
    while hz <= 20_000.0 {
        let b = VcoBlock::plain(hz, RATE);
        let got = measure_hz(&b, if hz < 10.0 { 3 } else { 200 });
        let err = cents(got, hz as f64);
        eprintln!("vco ideal {hz:>9.2} Hz: {err:+.4} cents");
        assert!(err.abs() < 0.05, "{hz} Hz off by {err} cents");
        hz *= 2.0;
    }
}

#[test]
fn vco_part_scale_error_is_in_the_sheets_range() {
    // Octave ratios between 100 Hz and 3.2 kHz at typical and worst drift.
    for (drift, limit) in [(0.5, 0.002), (1.0, 0.01)] {
        for seed in 1..40 {
            let p = VcoPart::new(seed, drift);
            let (lo, hi) = (p.hz(100.0, 0.0) as f64, p.hz(3200.0, 0.0) as f64);
            let per_octave = (hi / lo).log2() / 5.0;
            // Comparator delay is the only other term below 3.2 kHz.
            assert!((per_octave - 1.0).abs() < limit + 0.002, "seed {seed}: {per_octave}");
        }
    }
    assert_eq!(VcoPart::new(7, 0.0).hz(1000.0, 5.0).round(), 1000.0);
}

#[test]
fn vco_goes_flat_only_in_the_top_octaves() {
    let p = VcoPart::new(3, 0.5);
    let flat = |hz: f32| {
        // Three octaves measured downwards from `hz`: the scale error is flat, the delay is not.
        cents(p.hz(hz, 0.0) as f64, p.hz(hz / 8.0, 0.0) as f64) - 3600.0
    };
    eprintln!("vco flatness, 3 octaves ending at 2k {:+.2} 5k {:+.2} 10k {:+.2} 20k {:+.2} cents", flat(2000.0), flat(5000.0), flat(10000.0), flat(20000.0));
    assert!(flat(10_000.0) < flat(1000.0), "the top goes flat");
    assert!(flat(1000.0).abs() < flat(10_000.0).abs());
}

#[test]
fn vco_tempco_moves_pitch_by_the_sheets_figures() {
    // Drift 50 ppm per degree typical, 200 at most: at 10 degrees, under 3.5 cents.
    for seed in 1..40 {
        let p = VcoPart::new(seed, 1.0);
        let (a, b) = (p.hz(261.625, 0.0) as f64, p.hz(261.625, 10.0) as f64);
        assert!(cents(b, a).abs() < 3.6, "seed {seed}: {}", cents(b, a));
    }
}

fn run_vco(b: &VcoBlock, n: usize) -> Vec<f32> {
    let mut v = Vco::new();
    v.start(0.0);
    for _ in 0..4096 {
        v.tick(b, 0.0, &NO_EDGES);
    }
    (0..n).map(|_| v.tick(b, 0.0, &NO_EDGES)).collect()
}

#[test]
fn vco_levels_are_five_ten_and_twelve_volts() {
    // By the fundamental: a ramp of p-p P has P/pi, a triangle 4P/pi^2/... at
    // whole-cycle bins, free of the band limit's ringing.
    let (n, k) = (1usize << 16, 137usize);
    let hz = k as f32 * RATE / n as f32;
    let base = VcoBlock { saw: 0.0, ..VcoBlock::plain(hz, RATE) };
    let fund = |b: VcoBlock| spectrum(&run_vco(&b, n))[k];
    let pi = std::f64::consts::PI;
    let saw = fund(VcoBlock { saw: 1.0, ..base }) * pi;
    let tri = fund(VcoBlock { tri: 1.0, ..base }) * pi * pi / 4.0;
    let pulse = fund(VcoBlock { pulse: 1.0, ..base }) * pi / 2.0;
    eprintln!("vco p-p from the fundamental: saw {saw:.3} tri {tri:.3} pulse {pulse:.3} (10 V = 2.0)");
    assert!((saw - 2.0).abs() < 0.01);
    assert!((tri - 1.0).abs() < 0.01);
    assert!((pulse - 2.0 * PULSE_PEAK as f64).abs() < 0.01);
}

#[test]
fn vco_pwm_runs_zero_to_one_hundred_percent() {
    for width in [0.0f32, 0.1, 0.25, 0.5, 0.75, 0.9, 1.0] {
        let b = VcoBlock { saw: 0.0, pulse: 1.0, width, ..VcoBlock::plain(100.0, RATE) };
        let x = run_vco(&b, 48000);
        let duty = x.iter().filter(|v| **v > 0.0).count() as f32 / x.len() as f32;
        eprintln!("vco width {width:.2}: duty {duty:.4}");
        assert!((duty - width).abs() < 0.002, "{width}: {duty}");
    }
}

fn naive_saw(k: usize, n: usize) -> Vec<f32> {
    (0..n).map(|i| 2.0 * ((i * k % n) as f32 / n as f32) - 1.0).collect()
}

fn additive_saw(k: usize, n: usize) -> Vec<f32> {
    let mut out = vec![0.0f64; n];
    let mut h = 1;
    while h * k < n / 2 {
        let a = 2.0 / (std::f64::consts::PI * h as f64);
        let w = TAU * (h * k) as f64 / n as f64;
        for (i, o) in out.iter_mut().enumerate() {
            *o -= a * (w * i as f64).sin();
        }
        h += 1;
    }
    out.iter().map(|v| *v as f32).collect()
}

#[test]
fn vco_alias_measured_against_a_band_limited_reference() {
    let n = 1 << 16;
    // Whole bins: prime, so an image never lands on a harmonic by luck.
    for k in [1367usize, 4783, 9551, 16381] {
        let hz = k as f32 * RATE / n as f32;
        let b = VcoBlock::plain(hz, RATE);
        let x = run_vco(&b, n);
        let got = spectrum(&x);
        let (worst, total) = alias_of(&got, k, 20_000.0);
        let (naive_worst, naive_total) = alias_of(&spectrum(&naive_saw(k, n)), k, 20_000.0);
        // The reference: every harmonic below Nyquist.
        let reference = spectrum(&additive_saw(k, n));
        let mut off = 0.0f64;
        for h in 1.. {
            let f = h * k;
            if f as f64 * RATE as f64 / n as f64 > 15_000.0 {
                break;
            }
            off = off.max(db(got[f] / reference[f]).abs());
        }
        eprintln!("saw {hz:>8.1} Hz: alias worst {worst:.1} dB, total {total:.1} dB; plain core worst {naive_worst:.1} total {naive_total:.1}; harmonics to 15 kHz within {off:.2} dB of the reference");
        assert!(worst < -80.0, "{hz}: {worst}");
        assert!(worst < naive_worst - 30.0);
        assert!(off < 0.5, "{hz}: {off}");
    }
    for (name, mk) in [
        ("pulse 30%", VcoBlock { saw: 0.0, pulse: 1.0, width: 0.3, ..VcoBlock::plain(1.0, RATE) }),
        ("triangle", VcoBlock { saw: 0.0, tri: 1.0, ..VcoBlock::plain(1.0, RATE) }),
    ] {
        for k in [4783usize, 9551] {
            let hz = k as f32 * RATE / n as f32;
            let x = run_vco(&VcoBlock { inc: hz / RATE, ..mk }, n);
            let (worst, total) = alias_of(&spectrum(&x), k, 20_000.0);
            eprintln!("{name} {hz:>8.1} Hz: alias worst {worst:.1} dB total {total:.1} dB");
            assert!(worst < -70.0, "{name} {hz}: {worst}");
        }
    }
}

/// A slave under a master at bin `k`, the spectrum's alias; `base` the bin
/// its period's harmonics fall on (a reversal locks to a few master periods,
/// not one), dB re its strongest line.
fn sync_alias(sync: u8, k: usize, base: usize, ratio: f32) -> (f64, f64) {
    let n = 1 << 16;
    let hz = k as f32 * RATE / n as f32;
    let master_b = VcoBlock::plain(hz, RATE);
    let slave_b = VcoBlock { sync, ..VcoBlock::plain(hz * ratio, RATE) };
    let (mut m, mut s) = (Vco::new(), Vco::new());
    m.start(0.0);
    s.start(0.0);
    let mut x = vec![0.0; n];
    for i in 0..(48000 + n) {
        m.tick(&master_b, 0.0, &NO_EDGES);
        let y = s.tick(&slave_b, 0.0, &m.edges);
        if i >= 48000 {
            x[i - 48000] = y;
        }
    }
    let mag = spectrum(&x);
    let strongest = mag.iter().cloned().fold(0.0, f64::max);
    let (mut worst, mut sum) = (0.0f64, 0.0f64);
    for (i, m) in mag.iter().enumerate().skip(1) {
        if i % base != 0 {
            sum += m * m;
            if (i as f64) * (RATE as f64) / (n as f64) < 20_000.0 {
                worst = worst.max(*m);
            }
        }
    }
    (db(worst / strongest), db(sum.sqrt() / strongest))
}

/// A slave's output under a master, at `rate`.
fn sync_render(sync: u8, hz: f32, ratio: f32, rate: f32, n: usize) -> Vec<f32> {
    let master_b = VcoBlock::plain(hz, rate);
    let slave_b = VcoBlock { sync, ..VcoBlock::plain(hz * ratio, rate) };
    let (mut m, mut s) = (Vco::new(), Vco::new());
    m.start(0.0);
    s.start(0.0);
    (0..n)
        .map(|_| {
            m.tick(&master_b, 0.0, &NO_EDGES);
            s.tick(&slave_b, 0.0, &m.edges)
        })
        .collect()
}

fn hann_spectrum(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let w: Vec<f32> = x.iter().enumerate().map(|(i, v)| v * (0.5 - 0.5 * (TAU * i as f64 / n as f64).cos()) as f32).collect();
    spectrum(&w)
}

/// Reversal sync does not lock to the master (it moves the triangle by a
/// fixed distance: nothing draws it to an orbit), so alias is measured
/// against the same oscillator at 8x the rate, decimated: what it has where
/// that reference has nothing.
#[test]
fn vco_reversal_sync_alias_against_an_oversampled_reference() {
    let n = 1usize << 16;
    let os = 8usize;
    for (name, sync, hz, ratio) in [("hard", 1u8, 613.0f32, 2.5f32), ("hard_rise", 2, 613.0, 2.5), ("hard_fall", 3, 613.0, 2.5), ("hard", 1, 2203.0, 2.5)] {
        let ours = hann_spectrum(&sync_render(sync, hz, ratio, RATE, n + 512)[512..]);
        // The reference, through a long low-pass at 20 kHz.
        let hi = sync_render(sync, hz, ratio, RATE * os as f32, (n + 512) * os + 1024);
        let half = 512usize;
        let fir: Vec<f32> = (0..=2 * half)
            .map(|i| {
                let t = i as f64 - half as f64;
                let fc = 21_000.0 / (RATE as f64 * os as f64);
                let sinc = if t == 0.0 { 2.0 * fc } else { (TAU * fc * t).sin() / (std::f64::consts::PI * t) };
                let win = 0.42 + 0.5 * (std::f64::consts::PI * t / half as f64).cos() + 0.08 * (2.0 * std::f64::consts::PI * t / half as f64).cos();
                (sinc * win) as f32
            })
            .collect();
        let reference: Vec<f32> = (0..n + 512).map(|i| fir.iter().enumerate().map(|(k, c)| c * hi[i * os + k]).sum()).collect();
        let want = hann_spectrum(&reference[512..]);
        let peak = want.iter().cloned().fold(0.0, f64::max);
        let (mut empty, mut worst, mut floor) = (0usize, 0.0f64, 0.0f64);
        for i in 8..n / 2 {
            if i as f64 * RATE as f64 / n as f64 >= 19_000.0 {
                break;
            }
            if want[i] < peak * 1e-4 {
                empty += 1;
                worst = worst.max(ours[i]);
                floor = floor.max(want[i]);
            }
        }
        eprintln!("sync {name} at {hz} Hz: {empty} empty reference bins, ours at most {:.1} dB there (reference {:.1} dB)", db(worst / peak), db(floor / peak));
        assert!(empty > 1000, "{empty}");
        assert!(worst < peak * 10f64.powf(-65.0 / 20.0), "{name} {hz}: {}", db(worst / peak));
    }
}

#[test]
fn vco_reset_sync_alias() {
    // The conventional sync repeats with its master: anything off the
    // master's harmonics is alias.
    for k in [841usize, 3361] {
        let (worst, total) = sync_alias(4, k, k, 2.37);
        eprintln!("sync reset master bin {k}: alias worst {worst:.1} dB total {total:.1} dB");
        assert!(worst < -90.0, "{k}: {worst}");
    }
}

#[test]
fn vco_reversal_sync_acts_only_on_the_matching_slope() {
    // A rising edge reverses a rising triangle; a falling triangle ignores it.
    let b = VcoBlock { sync: 2, ..VcoBlock::plain(100.0, RATE) };
    let mut v = Vco::new();
    v.start(0.1);
    let before = v.triangle();
    let rise = Edges { rise: 0.5, ..NO_EDGES };
    v.tick(&b, 0.0, &rise);
    let mut after = v.triangle();
    assert!(after < before + 0.0001, "rising triangle turned round: {before} {after}");
    // Now falling: the same edge changes nothing.
    let t0 = after;
    v.tick(&b, 0.0, &rise);
    after = v.triangle();
    assert!(after < t0, "still falling");
}

#[test]
fn vco_soft_sync_locks_the_period_to_a_multiple() {
    // A master a little above half the slave's... the slave's period becomes
    // a whole number of master periods.
    let master_hz = 700.0;
    let m_b = VcoBlock::plain(master_hz, RATE);
    let s_b = VcoBlock { sync: 5, soft: 0.5, ..VcoBlock::plain(250.0, RATE) };
    let (mut m, mut s) = (Vco::new(), Vco::new());
    m.start(0.0);
    s.start(0.0);
    let (mut count, mut first, mut last) = (0, None, 0.0f64);
    for i in 0..96000u64 {
        m.tick(&m_b, 0.0, &NO_EDGES);
        s.tick(&s_b, 0.0, &m.edges);
        if s.edges.wrap >= 0.0 && i > 4800 {
            let t = i as f64 + s.edges.wrap as f64;
            first.get_or_insert(t);
            last = t;
            count += 1;
        }
    }
    let hz = (count - 1) as f64 / (last - first.unwrap()) * RATE as f64;
    let multiple = master_hz as f64 / hz;
    eprintln!("soft sync: slave {hz:.2} Hz, master / slave = {multiple:.4}");
    assert!((multiple - multiple.round()).abs() < 0.01, "{multiple}");
    assert!(hz > 250.0, "the threshold came down: {hz}");
}

#[test]
fn vco_linear_fm_stops_at_the_reference_current_and_never_reverses() {
    let b = VcoBlock { fm: 1.0, ..VcoBlock::plain(100.0, RATE) };
    let mut v = Vco::new();
    v.start(0.25);
    // Reach the coupling's rest, then pull the input fully negative: stopped.
    for _ in 0..24000 {
        v.tick(&b, 0.0, &NO_EDGES);
    }
    let t0 = v.triangle();
    for _ in 0..8 {
        v.tick(&VcoBlock { fm: 2.0, ..b }, -1.0, &NO_EDGES);
    }
    assert!((v.triangle() - t0).abs() < 1e-3, "stopped");
    // DC does not stay: the coupling lets it go.
    let hz = |dc: f32| {
        let mut v = Vco::new();
        v.start(0.0);
        let (mut c, mut first, mut last) = (0, None, 0.0);
        for i in 0..96000u64 {
            v.tick(&b, dc, &NO_EDGES);
            if v.edges.wrap >= 0.0 && i > 60000 {
                let t = i as f64 + v.edges.wrap as f64;
                first.get_or_insert(t);
                last = t;
                c += 1;
            }
        }
        (c - 1) as f64 / (last - first.unwrap()) * RATE as f64
    };
    eprintln!("lin fm with DC at the input: {:.3} Hz (nominal 100)", hz(0.4));
    assert!((hz(0.4) - 100.0).abs() < 0.5);
}

/// A sine through a filter: its gain at `f`, measured once settled.
fn gain(mode: u8, cutoff: f32, res: f32, f: f32, level: f32) -> f64 {
    let mut v = Vcf::new(1);
    v.prepare(mode, cutoff, res, 1.0, 0.0, RATE, 1);
    let settle = (RATE * 1.0) as usize;
    let n = (RATE * 0.5) as usize;
    let w = TAU * f as f64 / RATE as f64;
    let (mut c, mut s) = (0.0f64, 0.0f64);
    for i in 0..settle + n {
        let y = v.tick(level * (w * i as f64).sin() as f32) as f64;
        if i >= settle {
            c += y * (w * i as f64).cos();
            s += y * (w * i as f64).sin();
        }
    }
    (c * c + s * s).sqrt() * 2.0 / n as f64 / level as f64
}

#[test]
fn vcf_four_poles_minus_twelve_at_the_pole_frequency_at_every_octave() {
    let mut fc = 62.5f32;
    while fc < 10_000.0 {
        let g = db(gain(vcf::LP, fc, 0.0, fc, 0.05));
        eprintln!("vcf lp pole {fc:>8.1} Hz: {g:+.2} dB at the pole");
        assert!((g + 12.04).abs() < 0.4, "{fc}: {g}");
        fc *= 2.0;
    }
    // 24 dB per octave well above the pole.
    let (a, b) = (db(gain(vcf::LP, 400.0, 0.0, 1600.0, 0.1)), db(gain(vcf::LP, 400.0, 0.0, 3200.0, 0.1)));
    eprintln!("vcf lp slope {:.2} dB/oct", b - a);
    assert!((b - a + 24.0).abs() < 1.5);
    let passband = db(gain(vcf::LP, 8000.0, 0.0, 200.0, 0.05));
    assert!(passband.abs() < 0.1, "{passband}");
}

#[test]
fn vcf_loses_passband_as_resonance_rises() {
    // Oscillation needs a loop gain of 4, so the passband falls towards 1/5.
    for (res, expect) in [(0.0, 0.0), (0.5, -11.47), (0.9, -13.98)] {
        let g = db(gain(vcf::LP, 8000.0, res, 150.0, 0.03));
        eprintln!("vcf res {res:.1}: passband {g:+.2} dB, modelled {expect:+.2}");
        assert!((g - expect).abs() < 0.6, "{res}: {g}");
    }
}

/// The resonance's control curve (figure 6: 1 mS at 100 uA, bending over).
#[test]
fn vcf_resonance_rises_and_peaks_by_the_sheets_thirty_db() {
    let mut last = -100.0;
    for res in [0.2f32, 0.4, 0.6, 0.8, 0.85] {
        let peak = db(gain(vcf::LP, 1000.0, res, 1000.0, 0.02) / gain(vcf::LP, 8000.0, res, 150.0, 0.02));
        eprintln!("vcf res {res:.2}: {peak:+.2} dB at the pole, re the passband");
        assert!(peak > last);
        last = peak;
    }
    // Just below the oscillation point the sheet obtains 20 dB at least (30 typical).
    let near = db(gain(vcf::LP, 1000.0, 0.895, 1000.0, 0.002) / gain(vcf::LP, 8000.0, 0.895, 150.0, 0.002));
    eprintln!("vcf res 0.895: {near:+.2} dB");
    assert!(near > 20.0, "{near}");
}

/// Free-running output: its level and frequency once it has built up.
fn oscillation(mode: u8, cutoff: f32, res: f32) -> (f64, f64) {
    let mut v = Vcf::new(1);
    v.prepare(mode, cutoff, res, 1.0, 0.0, RATE, 1);
    let n = (RATE * 4.0) as usize;
    let mut last = 0.0f32;
    let (mut peak, mut first, mut stop, mut count) = (0.0f32, None, 0.0f64, 0usize);
    for i in 0..n {
        let y = v.tick(0.0);
        if i > n * 3 / 4 {
            peak = peak.max(y.abs());
            if last <= 0.0 && y > 0.0 {
                let t = i as f64 - 1.0 + (-last as f64) / (y - last) as f64;
                first.get_or_insert(t);
                stop = t;
                count += 1;
            }
        }
        last = y;
    }
    let hz = if count > 1 { (count - 1) as f64 / (stop - first.unwrap()) * RATE as f64 } else { 0.0 };
    (peak as f64, hz)
}

#[test]
fn vcf_oscillates_from_ninety_percent_control_and_more_control_is_more_sine() {
    let (below, _) = oscillation(vcf::LP, 1000.0, 0.87);
    let (above, hz) = oscillation(vcf::LP, 1000.0, 0.93);
    eprintln!("vcf res 0.87 peak {below:.5}; res 0.93 peak {above:.4} at {hz:.2} Hz");
    assert!(below < 0.01, "{below}");
    assert!(above > 0.05, "{above}");
    let (a, b, c) = (oscillation(vcf::LP, 1000.0, 0.92).0, oscillation(vcf::LP, 1000.0, 0.96).0, oscillation(vcf::LP, 1000.0, 1.0).0);
    eprintln!("vcf oscillation level: res .92 {a:.4}, .96 {b:.4}, 1 {c:.4}");
    assert!(a < b && b < c);
}

#[test]
fn vcf_oscillation_stays_on_the_pole_frequency_unclipped() {
    // Sheet: typically under 0.5% while below clipping. At the bottom the
    // resonance input's coupling capacitor (2.9 Hz) adds phase and it runs a
    // little sharp, as the circuit does.
    for (fc, limit) in [(110.0f32, 1.7), (440.0, 0.5), (1000.0, 0.5), (4000.0, 0.5), (12_000.0, 0.5)] {
        let (peak, hz) = oscillation(vcf::LP, fc, 0.96);
        let shift = (hz / fc as f64 - 1.0) * 100.0;
        eprintln!("vcf oscillation at pole {fc:>7.1}: {hz:>9.2} Hz ({shift:+.3}%), peak {peak:.3}");
        assert!(shift.abs() < limit, "{fc}: {shift}%");
    }
}

#[test]
fn vcf_wirings_resonate_where_the_sheet_says() {
    // "2.4 times higher" for the high-pass, "1/2.4 = .42 times lower" for
    // the band-pass and the all-pass.
    // Just over the oscillation point, so the first stage is not clipping
    // (clipped, the sheet warns of shifts over 5%).
    let ratio = |mode| {
        let (_, hz) = oscillation(mode, 1000.0, 0.93);
        hz / 1000.0
    };
    let (hp, bp, ap) = (ratio(vcf::HP), ratio(vcf::BP), ratio(vcf::AP));
    eprintln!("vcf resonance over lowpass's: hp {hp:.3} (sheet 2.4), bp {bp:.3} (sheet .42), ap {ap:.3} (sheet .42)");
    assert!((hp - 2.414).abs() < 0.1, "{hp}");
    assert!((bp - 0.414).abs() < 0.03, "{bp}");
    // Three all-pass stages and the first one's resistor input make 0.48;
    // the sheet's figure is "approximately".
    assert!((ap - 0.48).abs() < 0.03, "{ap}");
    // Each wiring's input resistor is chosen by the sheet's formula for the
    // loop gain it needs, so all of them start at 90% of the control.
    for mode in [vcf::HP, vcf::BP, vcf::AP] {
        assert!(oscillation(mode, 1000.0, 0.87).0 < 0.01, "{mode} below");
        assert!(oscillation(mode, 1000.0, 0.93).0 > 0.03, "{mode} above");
    }
}

#[test]
fn vcf_other_wirings_have_their_responses() {
    let g = |m, f| db(gain(m, 1000.0, 0.0, f, 0.05));
    eprintln!("vcf hp: 100 Hz {:+.1}, 10 kHz {:+.1}", g(vcf::HP, 100.0), g(vcf::HP, 10000.0));
    assert!(g(vcf::HP, 100.0) < -60.0);
    assert!(g(vcf::HP, 10_000.0).abs() < 1.0);
    eprintln!("vcf bp: 100 Hz {:+.1}, 1 kHz {:+.1}, 10 kHz {:+.1}", g(vcf::BP, 100.0), g(vcf::BP, 1000.0), g(vcf::BP, 10000.0));
    assert!(g(vcf::BP, 1000.0) > g(vcf::BP, 100.0) + 15.0);
    assert!(g(vcf::BP, 1000.0) > g(vcf::BP, 10_000.0) + 15.0);
    for f in [100.0, 1000.0, 10_000.0] {
        assert!(g(vcf::AP, f).abs() < 0.3, "all-pass flat at {f}: {}", g(vcf::AP, f));
    }
    // The state-variable wiring's outputs and its Q.
    let s = |m, f, r| db(gain(m, 1000.0, r, f, 0.02));
    eprintln!("vcf sv lp 100 {:+.1} 10k {:+.1}; hp 100 {:+.1} 10k {:+.1}; notch at pole {:+.1}", s(vcf::SV_LP, 100.0, 0.0), s(vcf::SV_LP, 10000.0, 0.0), s(vcf::SV_HP, 100.0, 0.0), s(vcf::SV_HP, 10000.0, 0.0), s(vcf::SV_NOTCH, 1000.0, 0.0));
    assert!(s(vcf::SV_LP, 100.0, 0.0).abs() < 0.5);
    assert!(s(vcf::SV_LP, 10_000.0, 0.0) < -35.0);
    assert!(s(vcf::SV_HP, 10_000.0, 0.0).abs() < 0.5);
    assert!(s(vcf::SV_NOTCH, 1000.0, 0.0) < -30.0);
    let (q0, q1) = (s(vcf::SV_BP, 1000.0, 0.0), s(vcf::SV_BP, 1000.0, 1.0));
    eprintln!("vcf sv bp at the pole: res 0 {q0:+.1} dB, res 1 {q1:+.1} dB");
    assert!(q1 > q0 + 20.0);
}

/// Harmonics of a sine through `f`, re the fundamental.
fn harmonics(level: f32, mut f: impl FnMut(f32) -> f32) -> Vec<f64> {
    let n = 1 << 15;
    let k = 683;
    let w = TAU * k as f64 / n as f64;
    for i in 0..n {
        f(level * (w * i as f64).sin() as f32);
    }
    let x: Vec<f32> = (0..n).map(|i| f(level * (w * i as f64).sin() as f32)).collect();
    let m = spectrum(&x);
    (1..8).map(|h| m[h * k] / m[k]).collect()
}

#[test]
fn vcf_passband_distortion_is_mostly_second_harmonic_at_the_sheets_level() {
    // 3 dB below clipping (input 1): 0.1% typical, 0.3% at most.
    let mut v = Vcf::new(1);
    v.prepare(vcf::LP, 2000.0, 0.0, 1.0, 0.5, RATE, 1);
    let h = harmonics(1.0, |x| v.tick(x));
    let thd = h[1..].iter().map(|a| a * a).sum::<f64>().sqrt() * 100.0;
    eprintln!("vcf 1 kHz under a 2 kHz pole: thd {thd:.3}%, h2 {:.3}% h3 {:.3}% (sheet typ 0.1, max 0.3)", h[1] * 100.0, h[2] * 100.0);
    assert!(thd > 0.05 && thd < 0.3, "{thd}");
    assert!(h[1] > h[2], "mostly second");
}

#[test]
fn vcf_overload_clips_the_first_stage_cutting_resonance_and_shifting_dc() {
    let small = gain(vcf::LP, 1000.0, 0.8, 1000.0, 0.02);
    let large = gain(vcf::LP, 1000.0, 0.8, 1000.0, 4.0);
    eprintln!("vcf resonance peak: small signal {:+.1} dB, overloaded {:+.1} dB", db(small), db(large));
    assert!(large < small * 0.5, "apparent resonance falls");
    let mut v = Vcf::new(1);
    v.prepare(vcf::LP, 300.0, 0.8, 1.0, 0.0, RATE, 1);
    let mut out = 0.0f32;
    for i in 0..48000 {
        let x = 6.0 * (TAU * 300.0 * i as f64 / 48000.0).sin() as f32;
        out = v.tick(x);
    }
    let shift = v.dc_shift();
    eprintln!("vcf dc shift under overload: {shift:+.4} of the 6 V unit ({:+.2} V), last out {out:.3}", shift * 6.0);
    assert!(shift.abs() > 0.005);
}

#[test]
fn vcf_control_feedthrough_is_near_the_sheets_figure() {
    // Features: -45 dB typical; 60 mV over the control range.
    let mut v = Vcf::new(3);
    let settle = |v: &mut Vcf, hz: f32| {
        v.prepare(vcf::LP, hz, 0.0, 1.0, 0.5, RATE, 32);
        let mut y = 0.0;
        for _ in 0..32 {
            y = v.tick(0.0);
        }
        y
    };
    let (a, b) = (settle(&mut v, 20.0), settle(&mut v, 20_000.0));
    let step = (a - b).abs() as f64;
    // Against the 12 V p-p swing, that is against output 2 * 1.414.
    let re = db(step / (2.0 * std::f64::consts::SQRT_2));
    eprintln!("vcf control sweep moves the output {step:.4} ({re:.1} dB re full swing)");
    assert!(re < -40.0 && re > -60.0, "{re}");
    let mut ideal = Vcf::new(3);
    ideal.prepare(vcf::LP, 20.0, 0.0, 1.0, 0.0, RATE, 32);
    ideal.tick(0.0);
    ideal.prepare(vcf::LP, 20_000.0, 0.0, 1.0, 0.0, RATE, 32);
    let y = ideal.tick(0.0);
    assert!(y.abs() < 0.001, "{y}");
}

#[test]
fn vcf_survives_any_modulation_and_never_goes_denormal() {
    use crate::dsp::Rng;
    for mode in 0..8u8 {
        let mut rng = Rng::new(0x1234 + mode as u32);
        let mut v = Vcf::new(mode as u32 + 1);
        let mut peak = 0.0f32;
        for i in 0..400_000 {
            if i % 1 == 0 {
                let hz = 5.0 * 4000f32.powf(rng.unit());
                let res = if rng.unit() < 0.5 { 1.0 } else { rng.unit() };
                let drive = 1.0 + 7.0 * rng.unit();
                v.prepare(mode, hz, res, drive, rng.unit(), RATE, 1);
            }
            let x = match (i >> 12) & 3 {
                0 => rng.bipolar() * 8.0,
                1 => if rng.unit() < 0.5 { 8.0 } else { -8.0 },
                2 => 0.0,
                _ => (i as f32 * 0.3).sin() * 4.0,
            };
            let y = v.tick(x);
            assert!(y.is_finite(), "mode {mode} sample {i}");
            assert!(y == 0.0 || y.abs() > f32::MIN_POSITIVE, "denormal out, mode {mode}: {y}");
            peak = peak.max(y.abs());
        }
        eprintln!("vcf mode {mode} stress peak {peak:.2}");
        assert!(peak < 40.0, "mode {mode}: {peak}");
        // Long silence after it: the output's noise floor, never subnormal.
        v.prepare(mode, 1000.0, 0.0, 1.0, 0.0, RATE, 1);
        for _ in 0..480_000 {
            let y = v.tick(0.0);
            assert!(y == 0.0 || y.abs() > f32::MIN_POSITIVE);
        }
    }
}

#[test]
fn vco_survives_extreme_modulation() {
    use crate::dsp::Rng;
    let mut rng = Rng::new(9);
    let (mut m, mut s) = (Vco::new(), Vco::new());
    let mut peak = 0.0f32;
    for i in 0..400_000 {
        let hz = 0.1 * 200_000f32.powf(rng.unit());
        let a = VcoBlock { saw: 1.0, tri: rng.unit(), pulse: rng.unit(), width: rng.unit(), ..VcoBlock::plain(hz, RATE) };
        let b = VcoBlock { sync: (i as u8) % 6, soft: rng.unit(), fm: rng.unit(), fm_exp: 4.0 * rng.unit(), sym: 0.45 + 0.1 * rng.unit(), ..a };
        let y0 = m.tick(&a, 0.0, &NO_EDGES);
        let y = s.tick(&b, y0, &m.edges);
        assert!(y.is_finite() && y0.is_finite(), "{i}");
        assert!(y == 0.0 || y.abs() > f32::MIN_POSITIVE, "denormal {y}");
        peak = peak.max(y.abs());
    }
    eprintln!("vco stress peak {peak:.2}");
    assert!(peak < 200.0);
}

fn vca_harmonics(idle: f32, trim: bool, level: f32) -> Vec<f64> {
    let mut v = Vca::new(1);
    v.prepare(&VcaBlock { idle, trim, drift: 0.0 }, [1.0, 1.0], 1);
    harmonics(level, |x| v.tick([x, x])[0])
}

#[test]
fn vca_second_harmonic_follows_figure_three() {
    // Untrimmed, 10 dB below clipping (signal 1), dB re the fundamental, read
    // off the graph.
    for (idle, sheet) in [(1.0, -40.0), (10.0, -41.0), (20.0, -44.0), (50.0, -52.0), (100.0, -58.0), (200.0, -65.0)] {
        let h2 = db(vca_harmonics(idle, false, 1.0)[1]);
        eprintln!("vca idle {idle:>5.0} uA: h2 {h2:.1} dB (figure {sheet:.0})");
        assert!((h2 - sheet).abs() < 3.5, "{idle}: {h2}");
    }
}

#[test]
fn vca_distortion_tables() {
    let thd = |idle, trim| {
        let h = vca_harmonics(idle, trim, 1.0);
        h[1..].iter().map(|a| a * a).sum::<f64>().sqrt() * 100.0
    };
    let rows = [("class B untrimmed", 1.0, false, 1.5, 5.0), ("class A untrimmed", 100.0, false, 0.3, 1.0), ("class B trimmed", 1.0, true, 0.2, 0.8), ("class A trimmed", 100.0, true, 0.05, 0.2)];
    for (name, idle, trim, typ, max) in rows {
        let t = thd(idle, trim);
        eprintln!("vca {name}: thd {t:.3}% (sheet typ {typ}, max {max})");
        assert!(t < max && t > typ * 0.4, "{name}: {t}");
    }
}

#[test]
fn vca_control_laws_and_range() {
    // Equal dB per unit of exponential control (3 mV per dB), over 120 dB.
    for c in [0.05f32, 0.2, 0.5, 0.8, 1.0] {
        let g = db(vca::law_gain(1, c, 120.0) as f64);
        let want = -(1.0 - c as f64) * 120.0;
        assert!((g - want).abs() < 0.01, "{c}: {g}");
    }
    // Linear: 0.3% typical over the range.
    for c in [0.01f32, 0.1, 0.5, 1.0] {
        assert_eq!(vca::law_gain(0, c, 120.0), c);
    }
    assert_eq!(vca::law_gain(0, -1.0, 120.0), 0.0);
    // Measured through the cell: gain tracks, down to -100 dB.
    for (g, want) in [(1.0f32, 0.0), (0.1, -20.0), (0.01, -40.0), (0.001, -60.0), (1e-5, -100.0)] {
        let mut v = Vca::new(1);
        v.prepare(&VcaBlock { idle: 100.0, trim: true, drift: 0.0 }, [g, g], 1);
        let m = harmonics(0.5, |x| v.tick([x, x])[0]);
        let _ = m;
        let got = {
            let mut v = Vca::new(1);
            v.prepare(&VcaBlock { idle: 100.0, trim: true, drift: 0.0 }, [g, g], 1);
            let n = 1 << 14;
            let k = 341;
            let x: Vec<f32> = (0..n).map(|i| v.tick([0.5 * (TAU * k as f64 * i as f64 / n as f64).sin() as f32; 2])[0]).collect();
            db(spectrum(&x)[k] / 0.5)
        };
        eprintln!("vca gain {g}: {got:.2} dB");
        assert!((got - want).abs() < 0.2, "{g}: {got}");
    }
}

#[test]
fn vca_control_feedthrough_grows_with_idle_current() {
    let step = |idle, trim| {
        let mut v = Vca::new(1);
        let b = VcaBlock { idle, trim, drift: 0.5 };
        v.prepare(&b, [1.0, 1.0], 1);
        let on = v.tick([0.0, 0.0])[0];
        v.prepare(&b, [0.0, 0.0], 1);
        let off = v.tick([0.0, 0.0])[0];
        (off - on).abs() as f64 * 95.0
    };
    for (idle, trim, sheet) in [(1.0, false, 0.2), (100.0, false, 7.0), (1.0, true, 0.01), (100.0, true, 1.0)] {
        let ua = step(idle, trim);
        eprintln!("vca feedthrough idle {idle} trim {trim}: {ua:.3} uA (sheet typ {sheet})");
        assert!((ua / sheet - 1.0).abs() < 0.05, "{ua}");
    }
}

#[test]
fn vca_clips_at_the_peak_cell_current() {
    let peak = |idle, g: f32| {
        let mut v = Vca::new(1);
        v.prepare(&VcaBlock { idle, trim: false, drift: 0.0 }, [g, g], 1);
        (0..1000).map(|i| v.tick([100.0 * (i as f32 * 0.05).sin(); 2])[0].abs()).fold(0.0f32, f32::max)
    };
    // Input plus output at unity gain: each carries half of 600 and 1400 uA.
    let (b, a) = (peak(1.0, 1.0), peak(100.0, 1.0));
    eprintln!("vca clip at unity: class B {:.0} uA, class A {:.0} uA at the output", b * 95.0, a * 95.0);
    assert!((b * 95.0 - 300.0).abs() < 30.0, "{b}");
    assert!((a * 95.0 - 700.0).abs() < 70.0, "{a}");
}

fn eg_blk() -> EgBlock {
    EgBlock { attack: 0.1, decay: 0.2, release: 0.3, sustain: 0.5, slow_gate: false, drift: 0.0 }
}

#[test]
fn eg_control_law_is_two_ms_to_twenty_seconds_in_decades() {
    assert!((eg::time_for_control(0.0) - 0.002).abs() < 1e-5);
    assert!((eg::time_for_control(5.0) - 20.0).abs() < 0.01);
    // Equal volts, equal ratio.
    let r1 = eg::time_for_control(2.0) / eg::time_for_control(1.0);
    let r2 = eg::time_for_control(4.0) / eg::time_for_control(3.0);
    assert!((r1 - r2).abs() < 1e-3);
    assert!(eg::TIME_MAX / eg::TIME_MIN >= 50_000.0);
}

#[test]
fn eg_attack_is_a_true_rc_curve_cut_at_the_peak() {
    let b = eg_blk();
    let mut e = Eg::new(1);
    e.gate_up();
    e.trigger();
    let dt = 0.001;
    let mut t = 0.0f32;
    let mut peak_at = None;
    while e.stage == EgStage::Attack {
        let y = e.advance(&b, dt);
        t += dt;
        if e.stage == EgStage::Attack {
            // Towards 6.5 V from 0, in units of the 5 V peak.
            let want = 1.3 * (1.0 - (-t / 0.1f32).exp());
            assert!((y - want).abs() < 1e-3, "{t}: {y} vs {want}");
        } else {
            peak_at = Some(t);
        }
    }
    // ln(6.5 / 1.5) time constants to the comparator.
    let want = 0.1 * (6.5f32 / 1.5).ln();
    eprintln!("eg peak at {:.4} s, RC says {want:.4}", peak_at.unwrap());
    assert!((peak_at.unwrap() - want).abs() < 0.002);
}

#[test]
fn eg_decay_and_release_approach_their_finals_exponentially() {
    let b = eg_blk();
    let mut e = Eg::new(1);
    e.gate_up();
    e.trigger();
    while e.stage == EgStage::Attack {
        e.advance(&b, 0.0005);
    }
    let start = e.level;
    let mut t = 0.0;
    for _ in 0..800 {
        let y = e.advance(&b, 0.001);
        t += 0.001;
        let want = 0.5 + (start - 0.5) * (-t / 0.2f32).exp();
        assert!((y - want).abs() < 2e-3, "{t}: {y} vs {want}");
    }
    assert!((e.level - 0.5).abs() < 0.01, "sustain is linear in control");
    e.gate_down();
    let from = e.level;
    let mut t = 0.0;
    for _ in 0..600 {
        let y = e.advance(&b, 0.001);
        t += 0.001;
        let want = from * (-t / 0.3f32).exp();
        assert!((y - want).abs() < 2e-3, "{t}: {y} vs {want}");
    }
    for _ in 0..4000 {
        e.advance(&b, 0.001);
    }
    assert_eq!(e.stage, EgStage::Idle);
    assert_eq!(e.level, 0.0);
}

#[test]
fn eg_one_control_sets_the_same_time_on_all_three_segments() {
    // Equal time constants: the same fraction gone in the same time.
    let tau = 0.25;
    let b = EgBlock { attack: tau, decay: tau, release: tau, sustain: 0.0, slow_gate: false, drift: 0.0 };
    let mut e = Eg::new(1);
    e.gate_up();
    e.trigger();
    while e.stage == EgStage::Attack {
        e.advance(&b, 0.001);
    }
    let a = e.level;
    let d = e.advance(&b, tau);
    e.gate_up();
    e.trigger();
    while e.stage == EgStage::Attack {
        e.advance(&b, 0.001);
    }
    e.gate_down();
    let r0 = e.level;
    let r = e.advance(&b, tau);
    eprintln!("eg after one time constant: decay {:.4}, release {:.4} (e^-1 = 0.3679)", d / a, r / r0);
    assert!((d / a - r / r0).abs() < 0.01);
    assert!((d / a - 0.3679).abs() < 0.01);
}

#[test]
fn eg_gate_and_trigger_are_the_sheets_waveforms() {
    let b = eg_blk();
    // A trigger while the gate is up: attack again from where it is.
    let mut e = Eg::new(1);
    e.gate_up();
    e.trigger();
    for _ in 0..2000 {
        e.advance(&b, 0.001);
    }
    assert!((e.level - 0.5).abs() < 0.01);
    e.trigger();
    assert_eq!(e.stage, EgStage::Attack);
    let y = e.advance(&b, 0.001);
    assert!(y > 0.5 && y < 0.52, "from where it is: {y}");
    // A trigger with the gate down does nothing.
    let mut e = Eg::new(1);
    e.trigger();
    assert_eq!(e.stage, EgStage::Idle);
    e.advance(&b, 0.01);
    assert_eq!(e.level, 0.0);
    // A gate with no trigger: straight up to the sustain level, no peak.
    let mut e = Eg::new(1);
    e.gate_up();
    let mut most = 0.0f32;
    for _ in 0..100 {
        most = most.max(e.advance(&b, 0.001));
    }
    eprintln!("eg gate alone, 100 ms: level {:.4}, peak {most:.4}", e.level);
    assert!((e.level - 0.5).abs() < 0.001);
    assert!(most < 0.51);
    // With the charge transistor disabled, up at the decay rate.
    let slow = EgBlock { slow_gate: true, ..b };
    let mut e = Eg::new(1);
    e.gate_up();
    let y = e.advance(&slow, 0.2);
    let want = 0.5 * (1.0 - (-1.0f32).exp());
    assert!((y - want).abs() < 1e-3, "{y} vs {want}");
    // Gate down during the attack: release from there.
    let mut e = Eg::new(1);
    e.gate_up();
    e.trigger();
    let y0 = e.advance(&b, 0.02);
    e.gate_down();
    let y1 = e.advance(&b, 0.3);
    assert!((y1 / y0 - (-1.0f32).exp()).abs() < 0.01, "{y0} {y1}");
}

#[test]
fn eg_segments_do_not_depend_on_the_control_block() {
    let b = eg_blk();
    let run = |steps: usize, dt: f32| {
        let mut e = Eg::new(1);
        e.gate_up();
        e.trigger();
        for _ in 0..steps {
            e.advance(&b, dt);
        }
        e.gate_down();
        e.advance(&b, 0.05);
        e.level
    };
    let (coarse, fine) = (run(50, 0.008), run(800, 0.0005));
    assert!((coarse - fine).abs() < 2e-3, "{coarse} {fine}");
}

#[test]
fn eg_final_voltages_are_the_sheets_at_typical_drift() {
    let b = EgBlock { drift: 0.5, ..eg_blk() };
    let mut e = Eg::new(5);
    e.gate_up();
    e.trigger();
    for _ in 0..5000 {
        e.advance(&b, 0.001);
    }
    // Sustain final voltage error: +10 mV typical on 5 V (+0.002), around a
    // peak held 4.7..5.3 V.
    let err = e.level - 0.5 * (1.0 + super::part(5, 3) * super::tol(0.5, 0.0, 0.02, 0.06));
    eprintln!("eg sustain final error {:.1} mV", err * 5000.0);
    assert!((err * 5000.0 - 10.0).abs() < 1.0);
}

#[test]
fn band_line_delay_is_its_half_length() {
    let mut l = super::blep::BandLine::default();
    let mut first = None;
    for i in 0..64 {
        if l.push(1.0).abs() > 0.5 && first.is_none() {
            first = Some(i);
        }
    }
    assert_eq!(first, Some(HALF));
}
