// Offline probe of the stock piano: prints the numbers a voicing iteration
// is judged by. No audio device, no files, no dependencies.
//
//   probe [--design name=value,...] [--shape key:partial:gain_db:decay_scale,...] [--keys a,b,...] [ref] [bands] [partials] [scan] [passage] [cpu]   (default: all)
//
// ref      the Salamander fixture's eleven metrics, model minus reference
// bands    per note and velocity: level per band, centroid, attack share
// partials per-partial level and two-stage decay of selected notes
// scan     every key A0..C6: level per band and key-to-key roughness
// raw, gain, decay   the same scan without the table, with only its gains,
//          with only its decay scales (never part of the default run)
// passage  a low chord and a chromatic bass descent through the preset path
// cpu      render cost of a pedalled passage, seconds of CPU per second
// fund     every key C3..C7: the fundamental's string modes and how it is
//          heard to decay (never part of the default run)
// ripple   partials 1-8 of F#3..C5: how far each envelope strays from a
//          two-stage decay, and the share of the sound off the partial lines
// uni      the unison per partial: mistuning against the in-phase drain,
//          the modes as built and what two coupled strings would give
//          (ripple and uni are never part of the default run)
// --keys   MIDI numbers in place of the key list of partials, env, phase,
//          modes, fund, ripple and uni; `env12` is env with twelve partials
use makepad_piano_model::{Piano, PianoEvent, TimedEvent, PIANO_PRESETS};

const FS: f64 = 48000.0;
const FIXTURE: &str = include_str!("../tests/data/salamander_v3.tsv");
const BANDS: [(f64, f64); 7] =
    [(20.0, 80.0), (80.0, 200.0), (200.0, 500.0), (500.0, 1500.0), (1500.0, 4000.0), (4000.0, 10000.0), (10000.0, 20000.0)];

fn name(key: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[key as usize % 12], key as i32 / 12 - 1)
}

/// `--design name=value,...` overrides on the calibrated instrument.
fn design() -> Option<makepad_piano_model::DesignParams> {
    let args: Vec<String> = std::env::args().collect();
    let list = args.iter().position(|a| a == "--design").and_then(|i| args.get(i + 1))?;
    let mut dp = makepad_piano_model::DesignParams::default();
    for pair in list.split(',') {
        let (name, value) = pair.split_once('=').expect("--design name=value,...");
        assert!(dp.set(name, value.parse().expect("design value")), "unknown design parameter {name}");
    }
    Some(dp)
}

/// `--keys a,b,...` (MIDI numbers) replaces a section's own key list.
fn keys_or(default: &[u8]) -> Vec<u8> {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().position(|a| a == "--keys").and_then(|i| args.get(i + 1)) {
        Some(list) => list.split(',').map(|k| k.parse().expect("--keys a,b,...")).collect(),
        None => default.to_vec(),
    }
}

fn stock(dry: bool) -> Piano {
    let preset = PIANO_PRESETS.iter().find(|p| p.is_default).unwrap();
    let mut p = match design() {
        Some(dp) => {
            let mut p = Piano::new_with_params_calibrated(FS as f32, &dp, makepad_piano_model::calibration_data::DEFAULT_CALIBRATION);
            p.apply_preset_live(preset);
            p
        }
        None => Piano::new_with_preset(FS as f32, preset),
    };
    // `--shape key:partial:gain_db:decay_scale,...` reshapes single partials
    let args: Vec<String> = std::env::args().collect();
    if let Some(list) = args.iter().position(|a| a == "--shape").and_then(|i| args.get(i + 1)) {
        for item in list.split(',') {
            let f: Vec<f64> = item.split(':').map(|x| x.parse().expect("--shape key:partial:gain_db:decay_scale")).collect();
            let n = f[1] as usize;
            let mut gain = vec![1.0f32; n];
            let mut scale = vec![1.0f32; n];
            gain[n - 1] = 10f32.powf(f[2] as f32 / 20.0);
            scale[n - 1] = f[3] as f32;
            p.debug_shape_partials(f[0] as u8, &gain, &scale);
        }
    }
    if dry {
        p.set_reverb_mix(0.0);
        p.set_early_reflection_level(0.0);
        p.set_soft_clip(false);
    }
    p
}

/// (seconds, event) script rendered in 256-sample blocks.
fn render(p: &mut Piano, script: &[(f64, PianoEvent)], secs: f64) -> (Vec<f32>, Vec<f32>) {
    let total = (secs * FS) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let evs: Vec<(usize, PianoEvent)> = script.iter().map(|&(t, e)| ((t * FS).round() as usize, e)).collect();
    let mut te = Vec::new();
    let mut pos = 0;
    while pos < total {
        let n = 256.min(total - pos);
        te.clear();
        for &(at, event) in &evs {
            if at >= pos && at < pos + n {
                te.push(TimedEvent { offset: (at - pos) as u32, event });
            }
        }
        p.process(&te, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

fn note(key: u8, vel: u8, secs: f64, dry: bool) -> (Vec<f32>, Vec<f32>) {
    render(&mut stock(dry), &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], secs)
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut m = n >> 1;
        while m >= 1 && j & m != 0 {
            j ^= m;
            m >>= 1;
        }
        j |= m;
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f64, 0.0f64);
            for k in 0..len / 2 {
                let (ar, ai) = (re[i + k], im[i + k]);
                let (br, bi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (tr, ti) = (br * cr - bi * ci, br * ci + bi * cr);
                re[i + k] = ar + tr;
                im[i + k] = ai + ti;
                re[i + k + len / 2] = ar - tr;
                im[i + k + len / 2] = ai - ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// Mean of the two channels' one-sided power spectra (periodic Hann), as
/// tools/acoustic.py and tests/acoustic_reference.rs define it.
fn spectrum(l: &[f32], r: &[f32]) -> (f64, Vec<f64>) {
    let n = l.len().next_power_of_two();
    let w: Vec<f64> = (0..l.len()).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / l.len() as f64).cos()).collect();
    let norm = n as f64 * w.iter().map(|x| x * x).sum::<f64>();
    let mut power = vec![0.0; n / 2 + 1];
    for ch in [l, r] {
        let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
        for (i, &v) in ch.iter().enumerate() {
            re[i] = v as f64 * w[i];
        }
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            let one = if k == 0 || k == n / 2 { 1.0 } else { 2.0 };
            power[k] += 0.5 * one * (re[k] * re[k] + im[k] * im[k]) / norm;
        }
    }
    (FS / n as f64, power)
}

fn band(s: &(f64, Vec<f64>), lo: f64, hi: f64) -> f64 {
    let a = (lo / s.0).ceil() as usize;
    let b = ((hi / s.0).ceil() as usize).min(s.1.len());
    if a >= b { 0.0 } else { s.1[a..b].iter().sum() }
}

fn db(x: f64) -> f64 {
    10.0 * x.max(1e-30).log10()
}

fn energy(l: &[f32], r: &[f32]) -> f64 {
    l.iter().zip(r).map(|(&a, &b)| 0.5 * ((a as f64).powi(2) + (b as f64).powi(2))).sum()
}

fn onset(l: &[f32], r: &[f32]) -> usize {
    let frame = 48;
    let end = l.len().min(24000);
    let p: Vec<f64> = (0..end / frame).map(|i| energy(&l[i * frame..(i + 1) * frame], &r[i * frame..(i + 1) * frame])).collect();
    let peak = p.iter().copied().fold(0.0, f64::max);
    p.iter().position(|&x| x > peak * 1e-4).unwrap_or(0) * frame
}

fn sect(x: &[f32], a: f64, b: f64) -> &[f32] {
    &x[((a * FS).round() as usize).min(x.len())..((b * FS).round() as usize).min(x.len())]
}

fn slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let (sx, sy) = (x.iter().sum::<f64>(), y.iter().sum::<f64>());
    let sxx: f64 = x.iter().map(|v| v * v).sum();
    let sxy: f64 = x.iter().zip(y).map(|(a, b)| a * b).sum();
    (n * sxy - sx * sy) / (n * sxx - sx * sx)
}

/// The fixture's eleven metrics (index 6, the register level, is the raw
/// 0-2 s RMS here; the caller turns it into dB re C4).
fn fixture_metrics(l: &[f32], r: &[f32], key: u8) -> [f64; 11] {
    let o = onset(l, r);
    let (l, r) = (&l[o..], &r[o..]);
    let spec = |a, b| spectrum(sect(l, a, b), sect(r, a, b));
    let mut m = [0.0; 11];
    for (i, (a, b)) in [(0.05, 0.1), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let total = band(&s, 20.0, 20000.0);
        m[2 * i] = db(band(&s, 500.0, 2000.0) / total);
        m[2 * i + 1] = db(band(&s, 2000.0, 8000.0) / total);
    }
    let f0 = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
    for (i, (a, b)) in [(0.05, 0.3), (1.0, 2.0)].into_iter().enumerate() {
        let s = spec(a, b);
        let p: Vec<f64> = (1..=6).map(|n| band(&s, (n as f64 - 0.4) * f0, (n as f64 + 0.4) * f0)).collect();
        m[4 + i] = db(p[0] / p[1..].iter().copied().fold(0.0, f64::max));
    }
    let ms = energy(sect(l, 0.0, 2.0), sect(r, 0.0, 2.0)) / (2.0 * FS);
    m[6] = ms.sqrt();
    m[7] = energy(sect(l, 0.0, 0.005), sect(r, 0.0, 0.005)) / energy(sect(l, 0.0, 0.05), sect(r, 0.0, 0.05));
    let times: Vec<f64> = (0..17).map(|i| 0.1 + i as f64 * 0.05).collect();
    let spectra: Vec<_> = times.iter().map(|&t| spec(t, t + 0.1)).collect();
    for (i, (lo, hi)) in [(20.0, 500.0), (500.0, 2000.0), (2000.0, 8000.0)].into_iter().enumerate() {
        let p: Vec<f64> = spectra.iter().map(|s| db(band(s, lo, hi).max(ms * 1e-15))).collect();
        m[8 + i] = -slope(&times, &p);
    }
    m
}

fn section_ref() {
    println!("== ref: model minus Salamander, dry (emid ehigh lmid lhigh p1c_e p1c_l reg onset dlow dmid dhigh), then register dB of the reference");
    let rows: Vec<(u8, u8, f64, [f64; 11])> = FIXTURE.lines().filter(|l| !l.starts_with('#')).skip(1).map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        (f[0].parse().unwrap(), f[1].parse().unwrap(), f[9].parse().unwrap(), std::array::from_fn(|i| f[10 + i].parse().unwrap()))
    }).collect();
    let model: Vec<[f64; 11]> = rows.iter().map(|r| { let (l, rr) = note(r.0, r.1, 2.6, true); fixture_metrics(&l, &rr, r.0) }).collect();
    let mut sum = [[0.0f64; 11]; 2];
    let mut cnt = [0.0f64; 2];
    for (i, row) in rows.iter().enumerate() {
        let c4 = rows.iter().position(|r| r.0 == 60 && r.1 == row.1).unwrap();
        let mut d = [0.0; 11];
        for k in 0..11 {
            let m = if k == 6 { 20.0 * (model[i][6] / model[c4][6]).log10() } else { model[i][k] };
            d[k] = m - row.3[k];
        }
        let g = if row.0 <= 48 { 0 } else { 1 };
        cnt[g] += 1.0;
        for k in 0..11 {
            sum[g][k] += d[k].abs();
        }
        println!("{:>3} v{:<3} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.3} {:+6.1} {:+6.1} {:+6.1} | ref reg {:+5.1} mid_e {:+5.1} mid_l {:+5.1}",
            name(row.0), row.1, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9], d[10], row.3[6], row.3[0], row.3[2]);
    }
    for (g, label) in ["mean|err| A0..C3", "mean|err| C4..C7"].iter().enumerate() {
        let s: Vec<String> = (0..11).map(|k| format!("{:.2}", sum[g][k] / cnt[g])).collect();
        println!("{label}: {}", s.join(" "));
    }
}

fn band_line(l: &[f32], r: &[f32], a: f64, b: f64) -> (String, f64, f64) {
    let s = spectrum(sect(l, a, b), sect(r, a, b));
    let total = band(&s, 20.0, 20000.0);
    let cols: Vec<String> = BANDS.iter().map(|&(lo, hi)| format!("{:6.1}", db(band(&s, lo, hi)))).collect();
    let (mut num, mut den) = (0.0, 0.0);
    for (k, &p) in s.1.iter().enumerate() {
        let f = k as f64 * s.0;
        if (20.0..20000.0).contains(&f) {
            num += f * p;
            den += p;
        }
    }
    (cols.join(" "), db(total), num / den.max(1e-30))
}

fn section_bands(keys: &[u8]) {
    println!("== bands: dry, dB per band (20-80 80-200 200-500 500-1.5k 1.5-4k 4-10k 10-20k) | total dB | centroid Hz, windows 0.02-0.5 s and 1-3 s; attack = energy 0-30 ms / 0-500 ms; lr = L/R correlation 0.1-2 s; peak");
    for &key in keys {
        for vel in [28u8, 68, 112] {
            let (l, r) = note(key, vel, 3.2, true);
            let o = onset(&l, &r);
            let (l, r) = (&l[o..], &r[o..]);
            let (e, et, ec) = band_line(l, r, 0.02, 0.5);
            let (t, tt, tc) = band_line(l, r, 1.0, 3.0);
            let att = energy(sect(l, 0.0, 0.03), sect(r, 0.0, 0.03)) / energy(sect(l, 0.0, 0.5), sect(r, 0.0, 0.5));
            let (sl, sr) = (sect(l, 0.1, 2.0), sect(r, 0.1, 2.0));
            let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
            for (&a, &b) in sl.iter().zip(sr) {
                ab += a as f64 * b as f64;
                aa += a as f64 * a as f64;
                bb += b as f64 * b as f64;
            }
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
            println!("{:>3} v{:<3} E {e} | {et:6.1} | {ec:6.0}   L {t} | {tt:6.1} | {tc:6.0}   att {att:.3} lr {:+.2} peak {peak:.3}",
                name(key), vel, ab / (aa * bb).sqrt().max(1e-30));
        }
    }
}

fn section_partials(keys: &[u8], vel: u8) {
    println!("== partials at v{vel}, dry: n, Hz, level dB (0.1-0.6 s) > level dB (1-2 s), early slope dB/s (0.1-1.0 s), late slope dB/s (1.5-3.5 s); then the share of 0.3-1.3 s power that is NOT on a partial line");
    for &key in keys {
        let info = stock(true).key_info(key).unwrap();
        let (f0, b) = (info.f0 as f64, info.b_coeff as f64);
        let (l, r) = note(key, vel, 4.0, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let times: Vec<f64> = (0..70).map(|i| 0.05 + 0.05 * i as f64).collect();
        let spectra: Vec<_> = times.iter().map(|&t| spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2))).collect();
        let lvl = spectrum(sect(l, 0.1, 0.6), sect(r, 0.1, 0.6));
        let late = spectrum(sect(l, 1.0, 2.0), sect(r, 1.0, 2.0));
        let half = (0.3 * f0).min(12.0);
        println!("-- {} f0 {:.2} B {:.3e} strings {} partials {}", name(key), f0, b, info.n_strings, info.n_partials);
        let mut line = String::new();
        let shown = [1usize, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 18, 20, 24, 28, 32, 40, 48, 64, 80, 100];
        for &n in shown.iter().filter(|&&n| n <= info.n_partials) {
            let nf = n as f64;
            let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
            let series: Vec<f64> = spectra.iter().map(|s| db(band(s, f - half, f + half))).collect();
            let pick = |a: f64, z: f64| -> (Vec<f64>, Vec<f64>) {
                times.iter().zip(&series).filter(|(&t, _)| t >= a && t <= z).map(|(&t, &y)| (t, y)).unzip()
            };
            let (te, ye) = pick(0.1, 1.0);
            let (tl, yl) = pick(1.5, 3.5);
            line += &format!("{n}:{f:.0}Hz {:.1}>{:.1} {:.1}/{:.1}  ", db(band(&lvl, f - half, f + half)), db(band(&late, f - half, f + half)), -slope(&te, &ye), -slope(&tl, &yl));
        }
        println!("{line}");
        println!("   off-line share {:.1} dB", off_line_db(l, r, f0, b, info.n_partials));
    }
}

/// Share of the 0.3-1.3 s power (20 Hz - 12 kHz) that is not on a partial
/// line of the key's own series, dB.
fn off_line_db(l: &[f32], r: &[f32], f0: f64, b: f64, partials: usize) -> f64 {
    let s = spectrum(sect(l, 0.3, 1.3), sect(r, 0.3, 1.3));
    let total = band(&s, 20.0, 12000.0);
    let half = (0.3 * f0).min(12.0);
    let mut lines = 0.0;
    for n in 1..=partials {
        let nf = n as f64;
        let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
        if f < 12000.0 {
            lines += band(&s, f - half, f + half);
        }
    }
    db(((total - lines) / total).max(1e-12))
}

/// Envelope ripple of partials 1-8: the rms distance (dB) of a partial's
/// 0.1-2 s level, read every 50 ms through 200 ms windows, from a line fit
/// to each half of that span. A partial that decays in one or two stages
/// reads near zero; beats, holes and swells read as ripple.
fn section_ripple(keys: &[u8], vel: u8) {
    println!("== ripple v{vel}, dry: envelope ripple dB of partials 1..8 | mean of 1-4 | off-line share dB (0.3-1.3 s)");
    for &key in keys {
        let info = stock(true).key_info(key).unwrap();
        let (f0, b) = (info.f0 as f64, info.b_coeff as f64);
        let (l, r) = note(key, vel, 2.5, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let times: Vec<f64> = (0..39).map(|i| 0.1 + 0.05 * i as f64).collect();
        let spectra: Vec<_> = times.iter().map(|&t| spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2))).collect();
        let half = (0.3 * f0).min(12.0);
        let fit = |t: &[f64], y: &[f64]| -> f64 {
            let s = slope(t, y);
            let c = y.iter().sum::<f64>() / y.len() as f64 - s * t.iter().sum::<f64>() / t.len() as f64;
            t.iter().zip(y).map(|(&x, &v)| (v - (c + s * x)).powi(2)).sum::<f64>()
        };
        let ripple: Vec<f64> = (1..=8usize.min(info.n_partials)).map(|n| {
            let nf = n as f64;
            let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
            let y: Vec<f64> = spectra.iter().map(|s| db(band(s, f - half, f + half))).collect();
            let mid = times.len() / 2;
            ((fit(&times[..mid], &y[..mid]) + fit(&times[mid..], &y[mid..])) / times.len() as f64).sqrt()
        }).collect();
        let v: Vec<String> = ripple.iter().map(|x| format!("{x:4.1}")).collect();
        let low = ripple.iter().take(4).sum::<f64>() / ripple.len().min(4) as f64;
        println!("{:>3} {key:3} | {} | {low:4.2} | {:6.1}", name(key), v.join(" "), off_line_db(l, r, f0, b, info.n_partials));
    }
}

/// The unison per partial. Mistuning is the anti-phase mode's offset from
/// the in-phase line (rad/s); the in-phase drain g is what the bridge takes
/// from the strings moving together (the vertical mode's rate less the
/// anti-phase mode's); their ratio says whether the strings lock (well under
/// 1) or ring apart (over 1). Then the modes as built, and the modes of the
/// key's strings as one coupled system (unison::solve; vertical motion
/// only, the strings spread over that mistuning): the fastest mode, and
/// the slowest with its level under an even blow and under an uneven one
/// (1.15, 0.95, 0.90).
fn section_uni(keys: &[u8]) {
    use makepad_piano_model::unison::{solve, Unison};
    println!("== uni: per partial n:Hz | mistuning rad/s, drain g 1/s, lock ratio | built: decay dB/s vert/anti, anti weight dB | solved: fast dB/s and level dB, slow dB/s and level dB even / uneven");
    let p = stock(true);
    for &key in keys {
        let k = &p.keys_debug()[(key - 21) as usize];
        if k.n_osc != 3 {
            continue;
        }
        for m in 0..k.modes_per_osc.min(8) {
            let n = (m + 1) as f64;
            let f = n * k.f0 as f64 * (1.0 + k.b_coeff as f64 * n * n).sqrt();
            let slot = |o: usize| {
                let i = o * k.modes_padded + m;
                (-(k.cr_sus[i] as f64).hypot(k.ci_sus[i] as f64).ln() * FS, (k.gout[i] as f64).hypot(k.gout_re[i] as f64))
            };
            let (vert, anti) = (slot(0), slot(2));
            let mis = std::f64::consts::TAU * f * k.unison_detune;
            let g = (vert.0 - anti.0).max(1e-9);
            let e = if k.n_strings == 2 { [0.5 * mis, -0.5 * mis, 0.0] } else { [0.5 * mis, -0.1 * mis, -0.4 * mis] };
            let solved = |h: [f64; 3]| {
                let u = Unison { strings: k.n_strings, s: anti.0, hl: 1.0, e, e_pol: 0.0, g, pull: 0.0, chi: 0.0, eta: 0.0, pd: 0.0, rho: 0.0, h };
                let (modes, count) = solve(&u);
                let mut v: Vec<(f64, f64)> = modes[..count].iter().filter(|x| x.vert > 0.5).map(|x| (-8.686 * x.lambda.re, 20.0 * x.main.re.hypot(x.main.im).max(1e-9).log10())).collect();
                v.sort_by(|a, b| b.0.total_cmp(&a.0));
                (v[0], v[v.len() - 1])
            };
            let (fast, slow) = solved([1.0, 1.0, if k.n_strings == 3 { 1.0 } else { 0.0 }]);
            let (_, uneven) = solved(if k.n_strings == 3 { [1.15, 0.95, 0.90] } else { [1.1, 0.9, 0.0] });
            println!("{:>3} {}:{f:.0} | {mis:6.2} {g:6.2} {:6.2} | {:5.1}/{:4.1} {:5.0} | {:5.1} {:5.1}  {:5.1} {:6.1} / {:6.1}",
                name(key), m + 1, mis / g, 8.686 * vert.0, 8.686 * anti.0, 20.0 * (anti.1 / vert.1.max(1e-30)).max(1e-9).log10(),
                fast.0, fast.1, slow.0, slow.1, uneven.1);
        }
    }
}

/// The stock table with its gains zeroed or its decay scales neutral.
fn variant(mode: &str) -> Piano {
    use makepad_piano_model::calibration_data::DEFAULT_CALIBRATION;
    let preset = PIANO_PRESETS.iter().find(|p| p.is_default).unwrap();
    let mut p = match mode {
        "raw" => Piano::new_uncalibrated(FS as f32),
        "gain" | "decay" => {
            let mut notes = DEFAULT_CALIBRATION.to_vec();
            for n in &mut notes {
                if mode == "gain" {
                    n.decay_scale = [1.0; 240];
                } else {
                    n.gain_db = [[0.0; 240]; 3];
                }
            }
            Piano::new_with_calibration(FS as f32, &notes)
        }
        _ => Piano::new(FS as f32),
    };
    p.apply_preset_live(preset);
    p.set_reverb_mix(0.0);
    p.set_early_reflection_level(0.0);
    p.set_soft_clip(false);
    p
}

fn section_scan(mode: &str, vel: u8, table: bool) {
    println!("== scan {mode} v{vel}: every key, dry: rms dB 0-2 s, Hann 0.02-2 s bands (20-80 80-200 200-500 500-1.5k 1.5-4k 4-10k), centroid; late = total 1-2 s re 0.02-0.5 s");
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); 8];
    for key in 21u8..=84 {
        let (l, r) = render(&mut variant(mode), &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 2.3);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let rms = db(energy(sect(l, 0.0, 2.0), sect(r, 0.0, 2.0)) / (2.0 * FS));
        let s = spectrum(sect(l, 0.02, 2.0), sect(r, 0.02, 2.0));
        let (_, early, _) = band_line(l, r, 0.02, 0.5);
        let (_, late, cen) = band_line(l, r, 1.0, 2.0);
        let b: Vec<f64> = BANDS[..6].iter().map(|&(lo, hi)| db(band(&s, lo, hi))).collect();
        if table {
            let t: Vec<String> = b.iter().map(|x| format!("{x:6.1}")).collect();
            let att = energy(sect(l, 0.0, 0.03), sect(r, 0.0, 0.03)) / energy(sect(l, 0.0, 0.5), sect(r, 0.0, 0.5));
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
            println!("{:>3} {key:3} rms {rms:6.1} | {} | cen {cen:5.0} | late {:+5.1} | att {att:.2} peak {peak:.2}", name(key), t.join(" "), late - early);
        }
        cols[0].push(rms);
        cols[1].push(late - early);
        for (i, x) in b.iter().enumerate() {
            cols[2 + i].push(*x);
        }
    }
    // key-to-key roughness: a key against the mean of its two neighbours
    let rough = |c: &[f64], a: usize, z: usize| -> (f64, f64) {
        let d: Vec<f64> = (a.max(1)..z.min(c.len() - 1)).map(|i| c[i] - 0.5 * (c[i - 1] + c[i + 1])).collect();
        ((d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt(), d.iter().fold(0.0f64, |m, x| m.max(x.abs())))
    };
    for (label, a, z) in [("A0..B2", 0usize, 27usize), ("C3..B4", 27, 51), ("C5..C6", 51, 64)] {
        let (r0, m0) = rough(&cols[0], a, z);
        let (r1, m1) = rough(&cols[1], a, z);
        let (r3, m3) = rough(&cols[3], a, z);
        let (r4, m4) = rough(&cols[4], a, z);
        let (r5, m5) = rough(&cols[5], a, z);
        println!("roughness {mode} v{vel} {label}: rms level {r0:.2} (max {m0:.1}) | late-early {r1:.2} ({m1:.1}) | 80-200 {r3:.2} ({m3:.1}) | 200-500 {r4:.2} ({m4:.1}) | 500-1.5k {r5:.2} ({m5:.1}) dB");
    }
}

fn section_strike(keys: &[u8], vel: u8) {
    println!("== strike v{vel}: per-partial level dB over the first 120 ms, partials 1..16");
    for &key in keys {
        let info = stock(true).key_info(key).unwrap();
        let (f0, b) = (info.f0 as f64, info.b_coeff as f64);
        let (l, r) = note(key, vel, 0.6, true);
        let o = onset(&l, &r);
        let s = spectrum(sect(&l[o..], 0.0, 0.12), sect(&r[o..], 0.0, 0.12));
        let half = (0.3 * f0).min(12.0);
        let v: Vec<String> = (1..=16).map(|n| {
            let nf = n as f64;
            let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
            format!("{:.0}", db(band(&s, f - half, f + half)))
        }).collect();
        let (cols, total, _) = band_line(&l[o..], &r[o..], 0.0, 0.03);
        println!("{:>3} {} | first 30 ms bands {cols} total {total:.1}", name(key), v.join(" "));
    }
}

fn section_board() {
    println!("== board: steady-state radiation per third octave, 24 points each: board modes re instant paths (mean dB), and of the sum: ripple (dB rms about the band mean), deepest notch and highest peak re the mean");
    let p = stock(true);
    for key in [30u8, 50, 70, 90] {
        let mut line = format!("key {key}:");
        let mut f = 50.0f64;
        while f < 5000.0 {
            let pts: Vec<(f64, f64, f64)> = (0..24).map(|i| p.radiation_debug(key, f * (i as f64 / 72.0).exp2())).collect();
            let d = |x: f64| 20.0 * x.max(1e-30).log10();
            let ratio = pts.iter().map(|t| d(t.0) - d(t.1)).sum::<f64>() / 24.0;
            let tot: Vec<f64> = pts.iter().map(|t| d(t.2)).collect();
            let mean = tot.iter().sum::<f64>() / 24.0;
            let rms = (tot.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / 24.0).sqrt();
            let (lo, hi) = tot.iter().fold((0.0f64, 0.0f64), |(a, b), x| (a.min(x - mean), b.max(x - mean)));
            line += &format!("  {f:.0}Hz {ratio:+.0} r{rms:.1} {lo:.0}/{hi:+.0}");
            f *= (1.0f64 / 3.0).exp2();
        }
        println!("{line}");
    }
}

fn section_cal(keys: &[u8]) {
    println!("== cal: resolved mezzo gain dB per partial (1..), and its decay dB/s of the first string mode");
    let p = stock(true);
    for &key in keys {
        let c = p.calibration_debug(key).unwrap();
        let k = &p.keys_debug()[(key - 21) as usize];
        let g: Vec<String> = (0..60.min(k.modes_per_osc)).map(|m| format!("{:.0}", c.gain_db[1][m])).collect();
        let d: Vec<String> = (0..60.min(k.modes_per_osc)).map(|m| format!("{:.0}", -8.686 * (k.cr_sus[m] as f64).hypot(k.ci_sus[m] as f64).ln() * FS)).collect();
        // strike comb times the first string mode's bridge weight, gain included
        let a: Vec<String> = (0..60.min(k.modes_per_osc)).map(|m| {
            let amp = (k.gin[m] as f64).abs() * (k.gout[m] as f64).hypot(k.gout_re[m] as f64);
            format!("{:.0}", 20.0 * amp.log10() + c.gain_db[1][m] as f64 + 200.0)
        }).collect();
        println!("{:>3} gain  {}\n    decay {}\n    force {}", name(key), g.join(" "), d.join(" "), a.join(" "));
    }
}

/// Per partial: each string mode's decay (dB/s) and its bridge weight re the
/// first mode (dB): VERT, HORZ and, on unisons, ANTI.
fn section_modes(keys: &[u8]) {
    println!("== modes: per partial n:Hz  decay dB/s vert/horz/anti  weight re vert dB horz/anti");
    let p = stock(true);
    for &key in keys {
        let k = &p.keys_debug()[(key - 21) as usize];
        let mut line = format!("{:>3}:", name(key));
        for m in (0..k.modes_per_osc.min(24)).filter(|m| *m < 10 || m % 2 == 1) {
            let n = (m + 1) as f64;
            let f = n * k.f0 as f64 * (1.0 + k.b_coeff as f64 * n * n).sqrt();
            let slot = |o: usize| {
                let i = o * k.modes_padded + m;
                (-8.686 * (k.cr_sus[i] as f64).hypot(k.ci_sus[i] as f64).ln() * FS, (k.gout[i] as f64).hypot(k.gout_re[i] as f64))
            };
            let v = slot(0);
            let d: Vec<String> = (0..k.n_osc).map(|o| format!("{:.1}", slot(o).0)).collect();
            let w: Vec<String> = (1..k.n_osc).map(|o| format!("{:.0}", 20.0 * (slot(o).1 / v.1).max(1e-9).log10())).collect();
            line += &format!("  {}:{f:.0} {} {}", m + 1, d.join("/"), w.join("/"));
        }
        println!("{line}");
    }
}

/// Level of the first partials every 100 ms (200 ms Hann windows), dB.
fn section_env(keys: &[u8], vel: u8) {
    println!("== env v{vel}, dry: per partial, level dB at 0.0, 0.1 .. 2.9 s");
    for &key in keys {
        let info = stock(true).key_info(key).unwrap();
        let (f0, b) = (info.f0 as f64, info.b_coeff as f64);
        let (l, r) = note(key, vel, 3.3, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let spectra: Vec<_> = (0..30).map(|i| spectrum(sect(l, 0.1 * i as f64, 0.1 * i as f64 + 0.2), sect(r, 0.1 * i as f64, 0.1 * i as f64 + 0.2))).collect();
        let half = (0.3 * f0).min(12.0);
        let shown = if std::env::args().any(|a| a == "env12") { 12 } else { 4 };
        for n in 1..=shown.min(info.n_partials) {
            let nf = n as f64;
            let f = nf * f0 * (1.0 + b * nf * nf).sqrt();
            let v: Vec<String> = spectra.iter().map(|s| format!("{:.0}", db(band(s, f - half, f + half)))).collect();
            println!("{:>3} {n}: {}", name(key), v.join(" "));
        }
    }
}

/// Per partial, each string mode as the bridge sees it: level and phase of
/// its residue re VERT, its frequency re VERT and its decay; then the mean
/// power of the two channels (dB re the strike) every 100 ms, where the
/// prompt and the aftersound interfere by cos(a) (keys::spread_aftersound),
/// and how far that falls below the sum of the modes' powers.
fn section_phase(keys: &[u8]) {
    println!("== phase: per partial, slot horz/anti: level dB, phase deg, offset Hz re vert, decay dB/s | mean power dB re strike at 0.0, 0.1 .. 1.9 s | deepest re power sum dB at s | tap gain re bridge weight dB");
    let p = stock(true);
    for &key in keys {
        let k = &p.keys_debug()[(key - 21) as usize];
        for m in 0..k.modes_per_osc.min(8) {
            let spread = if m < k.side_modes { k.after_spread.clamp(0.0, 1.0) } else { 0.0 };
            let c = (1.0 - spread * spread).sqrt();
            let slot = |o: usize| {
                let i = o * k.modes_padded + m;
                let (cr, ci) = (k.cr_sus[i] as f64, k.ci_sus[i] as f64);
                let back = if o > 0 && c > 0.0 { 1.0 / c } else { 1.0 };
                // (residue re, residue im, sigma 1/s, omega rad/s)
                (k.gout[i] as f64 * back, k.gout_re[i] as f64 * back, -cr.hypot(ci).ln() * FS, ci.atan2(cr) * FS)
            };
            let v = slot(0);
            let mut line = format!("{:>3} {}:", name(key), m + 1);
            line += &format!(" vert {:.1} dB/s", 8.686 * v.2);
            let mut tap = 0.0f64;
            for o in 1..k.n_osc {
                let s = slot(o);
                let lvl = 20.0 * (s.0.hypot(s.1) / v.0.hypot(v.1)).max(1e-9).log10();
                let mut ph = (s.1.atan2(s.0) - v.1.atan2(v.0)).to_degrees();
                while ph > 180.0 { ph -= 360.0 }
                while ph < -180.0 { ph += 360.0 }
                line += &format!(" | {lvl:.0} dB {ph:+.0} deg {:+.3} Hz {:.1} dB/s", (s.3 - v.3) / std::f64::consts::TAU, 8.686 * s.2);
                if m < k.side_modes {
                    let j = (o - 1) * makepad_piano_model::keys::SIDE_MODES + m;
                    let w = (k.side_l_im[j] as f64).hypot(k.side_l_re[j] as f64).hypot((k.side_r_im[j] as f64).hypot(k.side_r_re[j] as f64));
                    tap = tap.max(w / s.0.hypot(s.1).max(1e-30));
                }
            }
            let at = |t: f64| {
                let (mut vr, mut vi, mut sr, mut si, mut pw) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for o in 0..k.n_osc {
                    let s = slot(o);
                    let (a, w) = ((-s.2 * t).exp(), (s.3 - v.3) * t);
                    let (re, im) = (a * (s.0 * w.cos() - s.1 * w.sin()), a * (s.0 * w.sin() + s.1 * w.cos()));
                    if o == 0 {
                        (vr, vi) = (re, im);
                    } else {
                        sr += re;
                        si += im;
                    }
                    pw += re * re + im * im;
                }
                (vr * vr + vi * vi + sr * sr + si * si + 2.0 * c * (vr * sr + vi * si), pw)
            };
            let z = at(0.0).0;
            let e: Vec<String> = (0..20).map(|i| format!("{:.0}", db(at(0.1 * i as f64).0 / z))).collect();
            let (mut worst, mut when) = (0.0f64, 0.0);
            for i in 0..300 {
                let t = 0.01 * i as f64;
                let (s, pw) = at(t);
                if db(s / pw) < worst {
                    worst = db(s / pw);
                    when = t;
                }
            }
            println!("{line} | {} | {worst:.0} at {when:.2} | {:+.0}", e.join(" "), 20.0 * tap.max(1e-9).log10());
        }
    }
}

/// The fundamental of every key from C3 to C7 at mezzo: its string modes as
/// built (decay dB/s vert/horz/anti, bridge weight of horz/anti re vert) and
/// as heard: decay over the fixture's 0.1-1.0 s windows, level at 1-2 s re
/// 0.05-0.3 s, and the fixture's fundamental-over-partials-2-6 ratio early
/// and late.
fn section_fund() {
    println!("== fund v68, dry: key | modes decay dB/s vert/horz/anti, weight dB horz/anti | heard: decay dB/s 0.1-1 s, late re early dB, p1/cluster early, late");
    let p = stock(true);
    for key in keys_or(&(48u8..=96).collect::<Vec<_>>()) {
        let k = &p.keys_debug()[(key - 21) as usize];
        let slot = |o: usize| {
            let i = o * k.modes_padded;
            (-8.686 * (k.cr_sus[i] as f64).hypot(k.ci_sus[i] as f64).ln() * FS, (k.gout[i] as f64).hypot(k.gout_re[i] as f64))
        };
        let v = slot(0);
        let d: Vec<String> = (0..k.n_osc).map(|o| format!("{:.1}", slot(o).0)).collect();
        let w: Vec<String> = (1..k.n_osc).map(|o| format!("{:.0}", 20.0 * (slot(o).1 / v.1).max(1e-9).log10())).collect();
        let (l, r) = note(key, 68, 2.6, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let f0 = k.f0 as f64 * (1.0 + k.b_coeff as f64).sqrt();
        let line = |a: f64, b: f64| { let s = spectrum(sect(l, a, b), sect(r, a, b)); db(band(&s, 0.6 * f0, 1.4 * f0)) };
        let times: Vec<f64> = (0..17).map(|i| 0.1 + i as f64 * 0.05).collect();
        let lv: Vec<f64> = times.iter().map(|&t| line(t, t + 0.1)).collect();
        let m = fixture_metrics(l, r, key);
        println!("{:>3} {key:3} | {} {} | {:5.1} {:+6.1} {:+6.1} {:+6.1}", name(key), d.join("/"), w.join("/"), -slope(&times, &lv), line(1.0, 2.0) - line(0.05, 0.3), m[4], m[5]);
    }
}

fn section_passage() {
    println!("== passage: preset path (room and limiter on)");
    let on = |key, velocity| PianoEvent::NoteOn { key, velocity };
    let chord: Vec<(f64, PianoEvent)> = [33u8, 40, 45, 49, 52].iter().map(|&k| (0.0, on(k, 84))).collect();
    let mut descent = Vec::new();
    for (i, key) in (21u8..=45).rev().enumerate() {
        descent.push((0.3 * i as f64, on(key, 76)));
        descent.push((0.3 * i as f64 + 0.28, PianoEvent::NoteOff { key }));
    }
    let mut mid = Vec::new();
    for (i, ch) in [[48u8, 55, 60, 64], [53, 57, 60, 65], [55, 59, 62, 67], [48, 52, 55, 60]].iter().enumerate() {
        for &k in ch {
            mid.push((1.2 * i as f64, on(k, 72)));
            mid.push((1.2 * i as f64 + 1.1, PianoEvent::NoteOff { key: k }));
        }
    }
    for (label, script, secs) in [("low chord A1 E2 A2 C#3 E3 v84", chord, 4.0), ("chromatic descent A2..A0 v76", descent, 8.5), ("mid chords C3..G4 v72", mid, 5.5)] {
        for dry in [true, false] {
            let (l, r) = render(&mut stock(dry), &script, secs);
            let (cols, total, cen) = band_line(&l, &r, 0.0, secs);
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
            println!("{label} {}: total {total:6.1} | {cols} | cen {cen:5.0} | peak {peak:.3}", if dry { "dry " } else { "room" });
        }
    }
}

fn section_cpu() {
    // A pedalled passage: sustain down, 48 notes across the compass over
    // 6 s, rung for 10 s in all.
    let mut script = vec![(0.0, PianoEvent::Sustain { value: 1.0 })];
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % 72) as u8;
        script.push((0.125 * i as f64, PianoEvent::NoteOn { key, velocity: 60 + ((i * 13) % 50) as u8 }));
    }
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut p = stock(false);
        let t = std::time::Instant::now();
        let out = render(&mut p, &script, 10.0);
        std::hint::black_box(&out);
        best = best.min(t.elapsed().as_secs_f64() / 10.0);
    }
    println!("== cpu: pedalled 48-note passage, best of 3: {:.4} s of CPU per second of audio ({:.1}x real time)", best, 1.0 / best);
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    for flag in ["--design", "--shape", "--keys"] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            println!("{flag} {}", args[i + 1]);
            args.drain(i..i + 2);
        }
    }
    let want = |s: &str| args.is_empty() || args.iter().any(|a| a == s);
    let probe = [21u8, 24, 28, 33, 36, 43, 48, 60, 67, 72];
    if want("ref") {
        section_ref();
    }
    if want("bands") {
        section_bands(&probe);
    }
    if want("partials") {
        section_partials(&keys_or(&if args.iter().any(|a| a == "c3") { vec![45, 48, 51] } else { vec![21, 24, 28, 33, 36, 43, 48, 60, 67] }), 68);
    }
    if want("scan") {
        section_scan("stock", 68, true);
        section_scan("stock", 28, false);
        section_scan("stock", 112, false);
    }
    if args.iter().any(|a| a == "scan112") {
        section_scan("stock", 112, true);
    }
    for mode in ["raw", "gain", "decay"] {
        if args.iter().any(|a| a == mode) {
            section_scan(mode, 68, true);
        }
    }
    if args.iter().any(|a| a == "board") {
        section_board();
    }
    if args.iter().any(|a| a == "strike") {
        section_strike(&[35, 36, 37, 38, 39, 40], 112);
        section_cal(&[36, 37, 39]);
    }
    if args.iter().any(|a| a == "cal") {
        section_cal(&[36, 37, 38, 39]);
    }
    if args.iter().any(|a| a == "env" || a == "env12") {
        section_env(&keys_or(&[36, 45, 48, 54, 60, 64, 69]), 68);
    }
    if args.iter().any(|a| a == "phase") {
        section_phase(&keys_or(&[36, 45, 48, 54, 60, 64, 69, 72]));
    }
    if args.iter().any(|a| a == "modes") {
        section_modes(&keys_or(&[21, 24, 30, 33, 36, 45, 48, 60, 69, 72]));
    }
    if args.iter().any(|a| a == "fund") {
        section_fund();
    }
    let middle = [54u8, 55, 57, 60, 62, 64, 66, 67, 69, 71, 72];
    if args.iter().any(|a| a == "ripple") {
        section_ripple(&keys_or(&middle), 68);
    }
    if args.iter().any(|a| a == "uni") {
        section_uni(&keys_or(&[48, 54, 57, 60, 64, 67, 69, 72, 84]));
    }
    if want("passage") {
        section_passage();
    }
    if want("cpu") {
        section_cpu();
    }
}
