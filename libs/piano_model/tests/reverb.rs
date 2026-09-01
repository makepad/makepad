// Numerical verification of the output stage's FDN reverb: decay time
// matches the setting, highs decay faster than lows, echo density grows into
// a dense tail, the stereo tails are decorrelated, and nothing can make it
// blow up.

mod common;

use common::*;
use makepad_piano_model::fx::{Reverb, ReverbParams, ReverbPreset};

fn impulse_response(rv: &mut Reverb, seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let n = (seconds * FS as f64) as usize;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    for k in 0..n {
        let x = if k == 0 { 1.0 } else { 0.0 };
        let (wl, wr) = rv.process(x, x);
        l[k] = wl;
        r[k] = wr;
    }
    (l, r)
}

/// RT60 from the log-energy slope of the Schroeder decay curve.
fn rt60(x: &[f32], t0: f64, t1: f64) -> f64 {
    // backwards-integrated energy
    let mut edc = vec![0.0f64; x.len()];
    let mut acc = 0.0f64;
    for k in (0..x.len()).rev() {
        acc += (x[k] as f64) * (x[k] as f64);
        edc[k] = acc;
    }
    let mut ts = Vec::new();
    let mut ys = Vec::new();
    let mut k = (t0 * FS as f64) as usize;
    while k < ((t1 * FS as f64) as usize).min(x.len()) {
        if edc[k] > 0.0 {
            ts.push(k as f64 / FS as f64);
            ys.push(10.0 * edc[k].log10());
        }
        k += 2400;
    }
    let slope = linreg_slope(&ts, &ys).unwrap(); // dB per second
    -60.0 / slope
}

#[test]
fn rt60_matches_setting() {
    for &decay in &[0.8f32, 2.0, 4.0] {
        let mut rv = Reverb::new(FS);
        rv.set_params(ReverbParams { decay_s: decay, size: 1.0, damping: 0.0, predelay_s: 0.0 });
        let (l, _) = impulse_response(&mut rv, decay as f64 * 2.5);
        let measured = rt60(&l, 0.15, decay as f64 * 1.5);
        println!("decay set {decay:.1} s -> measured RT60 {measured:.2} s");
        assert!(
            (measured - decay as f64).abs() / (decay as f64) < 0.25,
            "RT60 {measured:.2} does not match setting {decay}"
        );
    }
}

#[test]
fn highs_decay_faster_than_lows() {
    let mut rv = Reverb::new(FS);
    rv.set_params(ReverbParams { decay_s: 2.5, size: 1.0, damping: 0.6, predelay_s: 0.0 });
    let (l, _) = impulse_response(&mut rv, 4.0);
    let band_rt = |lo: f64, hi: f64| {
        // measure band energy decay across 300 ms windows
        let mut ts = Vec::new();
        let mut ys = Vec::new();
        for w in 0..8 {
            let t0 = 0.2 + w as f64 * 0.3;
            let seg = sec(&l, t0, t0 + 0.3);
            let (bin, ps) = power_spectrum(seg);
            let e = band_power(bin, &ps, lo, hi);
            if e > 0.0 {
                ts.push(t0 + 0.15);
                ys.push(10.0 * e.log10());
            }
        }
        -60.0 / linreg_slope(&ts, &ys).unwrap()
    };
    let rt_low = band_rt(150.0, 700.0);
    let rt_high = band_rt(3500.0, 9000.0);
    println!("band RT60: low {rt_low:.2} s, high {rt_high:.2} s");
    assert!(rt_low > rt_high * 1.4, "damping must make highs die faster (low {rt_low:.2} vs high {rt_high:.2})");
}

#[test]
fn echo_density_grows_dense() {
    let mut rv = Reverb::new(FS);
    rv.set_preset(ReverbPreset::ConcertHall);
    rv.set_params(ReverbParams { predelay_s: 0.0, ..rv.params() });
    let (l, _) = impulse_response(&mut rv, 1.0);
    // Abel-style normalized echo density: fraction of samples above the
    // local std, relative to the Gaussian expectation erfc(1/sqrt2)=0.3173.
    let density_at = |t: f64| {
        let seg = sec(&l, t, t + 0.02);
        let sd = rms(seg);
        if sd == 0.0 {
            return 0.0;
        }
        let frac = seg.iter().filter(|&&v| (v as f64).abs() > sd).count() as f64 / seg.len() as f64;
        frac / 0.3173
    };
    let d_early = density_at(0.012);
    let d_late = density_at(0.25);
    println!("normalized echo density: early {d_early:.2}, late {d_late:.2}");
    assert!(d_late > 0.6, "tail must become dense (late density {d_late:.2})");
    assert!(d_late > d_early * 1.5, "echo density must grow ({d_early:.2} -> {d_late:.2})");
}

#[test]
fn stereo_tails_are_decorrelated() {
    let mut rv = Reverb::new(FS);
    rv.set_preset(ReverbPreset::ConcertHall);
    let (l, r) = impulse_response(&mut rv, 3.0);
    let a = sec(&l, 0.5, 2.5);
    let b = sec(&r, 0.5, 2.5);
    let ea = rms(a);
    let eb = rms(b);
    println!("tail RMS L {ea:.3e} R {eb:.3e}");
    assert!(ea / eb < 1.6 && eb / ea < 1.6, "stereo tails must carry similar energy");
    let mut worst = 0.0f64;
    for lag in -96i64..=96 {
        let mut c = 0.0f64;
        for k in 0..a.len() {
            let j = k as i64 + lag;
            if j >= 0 && (j as usize) < b.len() {
                c += a[k] as f64 * b[j as usize] as f64;
            }
        }
        let norm = c / (ea * eb * a.len() as f64);
        worst = worst.max(norm.abs());
    }
    println!("max |normalized xcorr| over +/-2 ms: {worst:.3}");
    assert!(worst < 0.35, "stereo tails too correlated: {worst:.3}");
}

#[test]
fn reverb_never_blows_up() {
    let mut rv = Reverb::new(FS);
    rv.set_params(ReverbParams { decay_s: 12.0, size: 1.6, damping: 0.0, predelay_s: 0.1 });
    let mut rng = 0x8badf00du32;
    let mut peak_out = 0.0f64;
    let n = (10.0 * FS as f64) as usize;
    let mut last = (0.0, 0.0);
    for _ in 0..n {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        let x = (rng >> 8) as f32 * (1.0 / 8_388_608.0) - 1.0; // full-scale noise
        let (wl, wr) = rv.process(x, x);
        assert!(wl.is_finite() && wr.is_finite());
        peak_out = peak_out.max(wl.abs().max(wr.abs()) as f64);
        last = (wl, wr);
    }
    println!("10 s full-scale noise at max settings: peak {peak_out:.2}, last {last:?}");
    assert!(peak_out < 20.0, "reverb output unbounded: {peak_out}");
    // input stops -> energy must decay
    let mut e_early = 0.0f64;
    let mut e_late = 0.0f64;
    for k in 0..(20.0 * FS as f64) as usize {
        let (wl, wr) = rv.process(0.0, 0.0);
        let e = (wl * wl + wr * wr) as f64;
        if k < FS as usize {
            e_early += e;
        } else if k >= 19 * FS as usize {
            e_late += e;
        }
    }
    println!("tail energy first second {e_early:.3e}, 20th second {e_late:.3e}");
    assert!(e_late < e_early * 0.05, "tail does not decay");
}
