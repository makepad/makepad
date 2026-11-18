// Offline probe of the waveguide piano: the numbers an iteration is judged
// by. No audio device, no files written, no dependencies.
//
//   probe [design] [hammer] [ref] [partials] [scan] [body] [cpu]   (default: all)
//
// design   per key: string data, loop size, partial error against the stiff string
// hammer   contact time and peak force against velocity
// ref      the Salamander fixture's eleven metrics, model minus reference
// partials per-partial level early and late, and decay, of selected notes
// scan     every key: level per band, centroid, key-to-key roughness
// body     listening proxies: bass partial count, 200 Hz-1.5 kHz share over time
// cpu      render cost of a pedalled passage, seconds of CPU per second
// scoreboard  (only when named) both pianos in one process on one table:
//          the fixture's columns, envelope ripple, halo, pedal lift, CPU
// colour   (only when named) what colours the sound: long-term third-octave
//          spectra in stereo, mono and side for both pianos and for each
//          switch, limiter load, the phantoms' share of the top, partial 1
//          at the bridge and at the ears, the board's sampling, block size
// ab       (only when named) level-matched WAVs of three passages, as Score
//          plays them and with each switch, into $PROBE_OUT or $DEVHUB_KEEP
//
// rate=<Hz> renders at that sample rate (48000 by default; Score builds the
// instrument at the output device's rate).
//
// The analysis helpers (spectrum, onset, the fixture's metrics) are the ones
// of makepad-piano-model's probe, so both instruments are measured alike.
use makepad_piano_waveguide::design::{loop_partial, string_partial, tune};
use makepad_piano_waveguide::{BoardShape, PianoEvent, TimedEvent, Voicing, WaveguidePiano};

static RATE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();

/// The sample rate everything is rendered and measured at.
fn fs() -> f64 {
    *RATE.get().unwrap_or(&48000.0)
}
const FIXTURE: &str = include_str!("../../piano_model/tests/data/salamander_v3.tsv");
const BANDS: [(f64, f64); 7] =
    [(20.0, 80.0), (80.0, 200.0), (200.0, 500.0), (500.0, 1500.0), (1500.0, 4000.0), (4000.0, 10000.0), (10000.0, 20000.0)];

/// `name=value` arguments override the voicing and the board, for trying a
/// change without a rebuild of anything else.
fn settings() -> (Voicing, BoardShape) {
    let (mut v, mut b) = (Voicing::default(), BoardShape::default());
    for arg in std::env::args().skip(1) {
        let Some((name, value)) = arg.split_once('=') else { continue };
        let value: f64 = value.parse().expect("number");
        match name {
            "felt_exponent" => v.felt_exponent = value,
            "felt_hysteresis" => v.felt_hysteresis = value,
            "contact" => v.contact = value,
            "felt_ceiling" => v.felt_ceiling = value,
            "regulation_tolerance" => v.regulation_tolerance = value,
            "unison_cents" => v.unison_cents = value,
            "strike_unevenness" => v.strike_unevenness = value,
            "sideways" => v.sideways = value,
            "bridge_yield" => v.bridge_yield = value,
            "bridge_yield_above" => v.bridge_yield_above = value,
            "bridge_stiffness" => v.bridge_stiffness = value,
            "bridge_stiff_below" => v.bridge_stiff_below = value,
            "phantoms" => v.phantoms = value,
            "knock" => b.knock = value,
            "sideways_attach" => v.sideways_attach = value,
            "first_mode" => b.first_mode = value,
            "spacing" => b.spacing = value,
            "last_mode" => b.last_mode = value,
            "loss_low" => b.loss_low = value,
            "loss_high" => b.loss_high = value,
            "radiation_corner" => b.radiation_corner = value,
            "efficient_above" => b.efficient_above = value,
            "warmth" => b.warmth = value,
            "direct_corner" => b.direct_corner = value,
            "direct_gain" => b.direct_gain = value,
            "width" => b.width = value,
            "coincident_above" => b.coincident_above = value,
            "coupling" => b.coupling = value,
            "duplex" => b.duplex = value,
            "floor" => b.floor = value,
            "positions" => b.positions = value as usize,
            "ear_phase" => b.ear_phase = value,
            "phantom_direct" => v.phantom_direct = value,
            "string_loss_high" => v.string_loss_high = value,
            // applied after the build (see `build`) or read by main
            "rate" | "room" | "knee" | "callback" | "vel" | "span" | "master" => {}
            other => panic!("unknown setting {other}"),
        }
    }
    (v, b)
}

/// A setting given as `name=value` that is applied to a built instrument.
fn arg(name: &str) -> Option<f64> {
    std::env::args().skip(1).find_map(|a| a.strip_prefix(name).and_then(|v| v.strip_prefix('=')).map(|v| v.parse().expect("number")))
}

/// The instrument with this voicing and board; dry has no room, otherwise
/// the room is `room=` (the instrument's own level if not given). `knee=`
/// moves the output limiter's knee.
fn build(v: &Voicing, b: &BoardShape, dry: bool) -> WaveguidePiano {
    let mut p = WaveguidePiano::with_voicing(fs() as f32, v, b);
    if dry {
        p.set_room_mix(0.0);
    } else if let Some(room) = arg("room") {
        p.set_room_mix(room as f32);
    }
    if let Some(master) = arg("master") {
        p.set_master_gain(master as f32);
    }
    if let Some(knee) = arg("knee") {
        p.set_limit_knee(knee as f32);
    }
    p
}

fn stock(dry: bool) -> WaveguidePiano {
    let (v, b) = settings();
    build(&v, &b, dry)
}

/// (seconds, event) script rendered in 256-sample blocks.
fn render(p: &mut WaveguidePiano, script: &[(f64, PianoEvent)], secs: f64) -> (Vec<f32>, Vec<f32>) {
    let total = (secs * fs()) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let evs: Vec<(usize, PianoEvent)> = script.iter().map(|&(t, e)| ((t * fs()).round() as usize, e)).collect();
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

/// One note on an instrument at rest; the instrument is built once per
/// thread and reused.
fn note(key: u8, vel: u8, secs: f64, dry: bool) -> (Vec<f32>, Vec<f32>) {
    thread_local! {
        static PIANOS: std::cell::RefCell<[Option<WaveguidePiano>; 2]> = const { std::cell::RefCell::new([None, None]) };
    }
    PIANOS.with(|p| {
        let mut p = p.borrow_mut();
        let piano = p[dry as usize].get_or_insert_with(|| stock(dry));
        piano.reset();
        render(piano, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], secs)
    })
}

fn name(key: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[key as usize % 12], key as i32 / 12 - 1)
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
    (fs() / n as f64, power)
}

/// The share of the power above about `fc` Hz, through a smooth edge (a
/// second-order high-pass's power response): a partial just under `fc` counts
/// nearly as much as one just over it. Steep spectra make a sharp edge read
/// a partial crossing it as a step of many dB that nobody hears.
fn soft_share(s: &(f64, Vec<f64>), fc: f64) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for (k, &p) in s.1.iter().enumerate() {
        let f = k as f64 * s.0;
        if (20.0..20000.0).contains(&f) {
            num += p * f.powi(4) / (f.powi(4) + fc.powi(4));
            den += p;
        }
    }
    num / den.max(1e-300)
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
    &x[((a * fs()).round() as usize).min(x.len())..((b * fs()).round() as usize).min(x.len())]
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
    let ms = energy(sect(l, 0.0, 2.0), sect(r, 0.0, 2.0)) / (2.0 * fs());
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
fn section_design() {
    println!("== design: key f1 Hz, B, Z kg/s, strings, bridge R kg/s, allpass sections, delay; worst partial error in cents up to n=8, 16, 32, 64 (partials below 12 kHz)");
    let t0 = std::time::Instant::now();
    let p = stock(true);
    println!("building the instrument: {:.3} s", t0.elapsed().as_secs_f64());
    let t0 = std::time::Instant::now();
    let all: Vec<_> = p.scale().iter().map(|k| makepad_piano_waveguide::loop_filters(k, fs())).collect();
    println!("designing 88 keys: {:.3} s ({} sections in all)", t0.elapsed().as_secs_f64(), all.iter().map(|f| f.sections).sum::<usize>());
    for key in [21u8, 24, 28, 33, 36, 40, 41, 48, 60, 72, 84, 96, 108] {
        let k = &p.scale()[(key - 21) as usize];
        let f = makepad_piano_waveguide::loop_filters(k, fs());
        let t = tune(&f, k.f1, fs());
        let mut worst = [0.0f64; 4];
        for n in 1..=64usize {
            let want = string_partial(k.f1, k.b, n);
            if want > 12_000.0 {
                break;
            }
            let cents = 1200.0 * (loop_partial(&f, t, n, fs()) / want).log2();
            for (i, lim) in [8usize, 16, 32, 64].into_iter().enumerate() {
                if n <= lim {
                    worst[i] = worst[i].max(cents.abs());
                }
            }
        }
        println!("{:>3} f1 {:8.2} B {:.2e} Z {:5.2} x{} R {:6.0} sections {:2} delay {:4} | cents {:6.2} {:6.2} {:6.2} {:6.2} | hammer {:.1} g p {:.2} | long {:5.0} Hz",
            name(key), k.f1, k.b, k.z, k.strings, k.bridge_r, f.sections, t.delay, worst[0], worst[1], worst[2], worst[3], k.hammer_mass * 1e3, k.hammer_p, k.longitudinal);
    }
}

fn section_ref() {
    println!("== ref: model minus Salamander, dry (emid ehigh lmid lhigh p1c_e p1c_l reg onset dlow dmid dhigh), then the reference's register, early and late mid share");
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
        println!("{:>3} v{:<3} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.1} {:+6.3} {:+6.1} {:+6.1} {:+6.1} | ref reg {:+5.1} mid_e {:+5.1} mid_l {:+5.1} | rms {:6.1} dB (ref {:6.1})",
            name(row.0), row.1, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9], d[10], row.3[6], row.3[0], row.3[2], 20.0 * model[i][6].log10(), 20.0 * row.2.log10());
    }
    for (g, label) in ["mean|err| A0..C3", "mean|err| C4..C7"].iter().enumerate() {
        let s: Vec<String> = (0..11).map(|k| format!("{:.2}", sum[g][k] / cnt[g])).collect();
        println!("{label}: {}", s.join(" "));
    }
}

/// Power of partial `n` (a band of 0.8 f1 around the stiff string's partial).
fn partial_power(s: &(f64, Vec<f64>), f1: f64, b: f64, n: usize) -> f64 {
    let f = string_partial(f1, b, n);
    band(s, f - 0.4 * f1, f + 0.4 * f1)
}

fn section_partials(keys: &[u8], vel: u8) {
    println!("== partials v{vel}: level dB of partials 1..16 at 0.05-0.35 s, then at 1.5-2.5 s, then decay dB/s between them");
    let p = stock(true);
    for &key in keys {
        let k = p.scale()[(key - 21) as usize].clone();
        let (l, r) = note(key, vel, 2.7, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let early = spectrum(sect(l, 0.05, 0.35), sect(r, 0.05, 0.35));
        let late = spectrum(sect(l, 1.5, 2.5), sect(r, 1.5, 2.5));
        let e: Vec<f64> = (1..=16).map(|n| db(partial_power(&early, k.f1, k.b, n))).collect();
        let t: Vec<f64> = (1..=16).map(|n| db(partial_power(&late, k.f1, k.b, n))).collect();
        let fmt = |v: &[f64]| v.iter().map(|x| format!("{x:6.1}")).collect::<Vec<_>>().join(" ");
        let d: Vec<f64> = e.iter().zip(&t).map(|(a, b)| (a - b) / 1.8).collect();
        println!("{:>3} early {}\n    late  {}\n    decay {}", name(key), fmt(&e), fmt(&t), fmt(&d));
    }
}

fn section_hammer() {
    println!("== touch: per key and velocity, dry: contact ms, peak force N, rms dB 0-2 s, centroid Hz at 0.02-0.5 s, share of energy above 2 kHz dB");
    let mut p = stock(true);
    for key in [21u8, 36, 48, 60, 72, 84, 96] {
        let mut line = String::new();
        for vel in [16u8, 28, 68, 112, 127] {
            p.reset();
            let (l, r) = render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 2.2);
            let (contact, force) = p.last_blow(key);
            line += &format!(" | v{vel:<3} {:4.2} ms {force:5.1} N", contact * 1e3);
            let o = onset(&l, &r);
            let (l, r) = (&l[o..], &r[o..]);
            let rms = db(energy(sect(l, 0.0, 2.0), sect(r, 0.0, 2.0)) / (2.0 * fs()));
            let s = spectrum(sect(l, 0.02, 0.5), sect(r, 0.02, 0.5));
            let (_, _, cen) = band_line(l, r, 0.02, 0.5);
            line += &format!(" {rms:6.1} {cen:5.0} {:+5.1}", db(band(&s, 2000.0, 20000.0) / band(&s, 20.0, 20000.0)));
        }
        println!("{:>3}{line}", name(key));
    }
}

fn section_scan(vel: u8, table: bool) {
    println!("== scan v{vel}: every key, dry: rms dB 0-2 s, Hann 0.02-2 s bands (20-80 80-200 200-500 500-1.5k 1.5-4k 4-10k), centroid; late = total 1-2 s re 0.02-0.5 s");
    let mut cols: Vec<Vec<f64>> = vec![Vec::new(); 8];
    let mut off: Vec<f64> = Vec::new();
    for key in 21u8..=108 {
        let (l, r) = note(key, vel, 2.3, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let rms = db(energy(sect(l, 0.0, 2.0), sect(r, 0.0, 2.0)) / (2.0 * fs()));
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
        // the fixture's register trend, dB re C4
        let target = makepad_piano_waveguide::scale::curve(&[(21.0, 3.0), (33.0, 2.5), (36.0, 1.0), (48.0, 0.0), (60.0, 0.0), (72.0, -2.5), (84.0, -6.0), (96.0, -12.0), (108.0, -18.0)], key as f64);
        off.push(rms - target);
        cols[0].push(rms);
        cols[1].push(late - early);
        for (i, x) in b.iter().enumerate() {
            cols[2 + i].push(*x);
        }
    }
    // Level against the register trend, averaged over seven keys around
    // each anchor, re the anchor at C4: what a per-key trim would correct.
    let around = |key: usize| -> f64 {
        let (a, z) = (key.saturating_sub(24), (key - 21 + 4).min(88));
        off[a..z].iter().sum::<f64>() / (z - a) as f64
    };
    let anchors: Vec<String> = (21..=108).step_by(6).map(|k| format!("{k}:{:+.1}", around(k) - around(60))).collect();
    println!("level v{vel} re register trend, re C4: {}  (C4 rms {:.1} dB)", anchors.join(" "), cols[0][39]);
    let rough = |c: &[f64], a: usize, z: usize| -> (f64, f64) {
        let d: Vec<f64> = (a.max(1)..z.min(c.len() - 1)).map(|i| c[i] - 0.5 * (c[i - 1] + c[i + 1])).collect();
        ((d.iter().map(|x| x * x).sum::<f64>() / d.len() as f64).sqrt(), d.iter().fold(0.0f64, |m, x| m.max(x.abs())))
    };
    for (label, a, z) in [("A0..B2", 0usize, 27usize), ("C3..B4", 27, 51), ("C5..C6", 51, 64), ("C#6..C8", 64, 88)] {
        let (r0, m0) = rough(&cols[0], a, z);
        let (r1, m1) = rough(&cols[1], a, z);
        let (r3, m3) = rough(&cols[3], a, z);
        let (r4, m4) = rough(&cols[4], a, z);
        let (r5, m5) = rough(&cols[5], a, z);
        println!("roughness v{vel} {label}: rms level {r0:.2} (max {m0:.1}) | late-early {r1:.2} ({m1:.1}) | 80-200 {r3:.2} ({m3:.1}) | 200-500 {r4:.2} ({m4:.1}) | 500-1.5k {r5:.2} ({m5:.1}) dB");
    }
}

fn section_body() {
    println!("== body v68: bass = partials within 30 dB of the strongest at 0.05-0.5 s, and the highest of them; body = share of 200 Hz-1.5 kHz in dB at 0.02-0.3 s, 0.3-1 s, 1-2 s, 2-3 s; level of each window re the first");
    let p = stock(true);
    for key in [21u8, 24, 28, 33, 36, 40, 43] {
        let k = p.scale()[(key - 21) as usize].clone();
        let (l, r) = note(key, 68, 1.0, true);
        let o = onset(&l, &r);
        let s = spectrum(sect(&l[o..], 0.05, 0.5), sect(&r[o..], 0.05, 0.5));
        let levels: Vec<f64> = (1..=120).map(|n| if string_partial(k.f1, k.b, n) < 16_000.0 { db(partial_power(&s, k.f1, k.b, n)) } else { -300.0 }).collect();
        let top = levels.iter().copied().fold(f64::MIN, f64::max);
        let strong: Vec<usize> = (0..levels.len()).filter(|&i| levels[i] > top - 30.0).collect();
        let loudest = levels.iter().position(|&x| x == top).unwrap() + 1;
        println!("{:>3} bass: {} partials within 30 dB, up to n={} ({:.0} Hz); strongest n={loudest}", name(key), strong.len(), strong.last().unwrap() + 1, string_partial(k.f1, k.b, strong.last().unwrap() + 1));
    }
    for key in [48u8, 53, 57, 60, 64, 67, 72, 77, 84] {
        let (l, r) = note(key, 68, 3.2, true);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let mut line = String::new();
        let mut first = 0.0;
        for (i, (a, b)) in [(0.02, 0.3), (0.3, 1.0), (1.0, 2.0), (2.0, 3.0)].into_iter().enumerate() {
            let s = spectrum(sect(l, a, b), sect(r, a, b));
            let total = band(&s, 20.0, 20000.0);
            if i == 0 {
                first = total;
            }
            line += &format!(" | share {:+5.1} level {:+6.1}", db(band(&s, 200.0, 1500.0) / total), db(total / first));
        }
        println!("{:>3} body{line}", name(key));
    }
}

fn section_board() {
    println!("== board: level dB of a bridge region's impulse response per third octave (25 Hz .. 8 kHz), regions bass to treble");
    let mut p = stock(true);
    let centres: Vec<f64> = (0..26).map(|i| 25.0 * 2f64.powf(i as f64 / 3.0)).collect();
    println!("      Hz {}", centres.iter().map(|c| format!("{c:5.0}")).collect::<Vec<_>>().join(" "));
    for bus in 0..4 {
        let (l, r) = p.board_impulse(bus, 65536);
        let s = spectrum_flat(&l, &r);
        let row: Vec<String> = centres.iter().map(|c| format!("{:5.1}", db(band(&s, c / 1.122, c * 1.122) / (c * 0.2316 / s.0)))).collect();
        println!("region {bus} {}", row.join(" "));
    }
    println!("== bridge v68: level dB of partials 1..16 of the bridge velocity at 0.05-0.35 s, before the board; then the board's gain dB at each");
    for key in [21u8, 28, 36, 45, 48, 55, 60, 69, 72, 84] {
        let k = p.scale()[(key - 21) as usize].clone();
        let v = p.bridge_velocity(key, 68, 24000);
        let s = spectrum(sect(&v, 0.05, 0.35), sect(&v, 0.05, 0.35));
        let e: Vec<String> = (1..=16).map(|n| format!("{:6.1}", db(partial_power(&s, k.f1, k.b, n)))).collect();
        let g: Vec<String> = (1..=16).map(|n| { let (l, r) = p.board_gain(key, string_partial(k.f1, k.b, n)); format!("{:6.1}", db(0.5 * (l + r))) }).collect();
        println!("{:>3} {}\n    {}", name(key), e.join(" "), g.join(" "));
    }
}

/// Where the board colours each key: per key at v60, the attack share
/// above 4 kHz over the first 50 ms at the bridge and radiated, then what
/// the board does to each band there (radiated minus bridge, dB, regulation
/// gain included): the low partials (under 2 kHz) and 4-8 kHz; the board's
/// own power gain at partials 1 and 2 and over 4-8 kHz. Each column also as
/// its deviation from the median of the key and its four nearest
/// neighbours. Then the board's raw gain at partial 1 per ear, and the
/// level over 0-0.5 s, whole and of partial 1 alone.
/// A summary over C4-C7 follows: spread (rms deviation from the 5-key
/// median), mean and worst neighbour step of the level, of partial 1 and of
/// the attack share; mean and worst step of the 4-8 kHz change; the
/// correlation of the two ears' deviations at partial 1; and the level's
/// trend (5-key mean) per register.
fn section_boardkeys() {
    let mut p = stock(true);
    let from: u8 = std::env::var("BK_FROM").ok().and_then(|x| x.parse().ok()).unwrap_or(58);
    let to: u8 = std::env::var("BK_TO").ok().and_then(|x| x.parse().ok()).unwrap_or(98);
    let len = (0.05 * fs()) as usize;
    let mut rows: Vec<(u8, [f64; 8])> = Vec::new();
    let mut more: Vec<[f64; 9]> = Vec::new();
    for key in from..=to {
        let k = p.scale()[(key - 21) as usize].clone();
        let v = p.bridge_velocity(key, 60, len);
        let sb = spectrum(&v, &v);
        p.reset();
        let (l, r) = render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: 60 })], 0.5);
        let sr = spectrum(sect(&l, 0.0, 0.05), sect(&r, 0.0, 0.05));
        let whole = spectrum(&l, &r);
        let gain = |hz: f64| { let (l, r) = p.board_gain(key, hz); 0.5 * (l + r) };
        let high_gain = (0..40).map(|i| gain(4000.0 * 2f64.powf(i as f64 / 40.0))).sum::<f64>() / 40.0;
        let f1 = string_partial(k.f1, k.b, 1);
        let t = p.board_transfer(key, f1, false);
        let side = db(partial_power(&spectrum(&l, &l), k.f1, k.b, 1)) - db(partial_power(&spectrum(&r, &r), k.f1, k.b, 1));
        let reg = p.regulation(key);
        more.push([db(t[0].0 * t[0].0 + t[0].1 * t[0].1), db(t[1].0 * t[1].0 + t[1].1 * t[1].1), db(energy(&l, &r) / l.len() as f64), db(partial_power(&whole, k.f1, k.b, 1)), side, reg[0] as f64, reg[1] as f64, db(band(&sr, 4000.0, 8000.0)), db(band(&sb, 4000.0, 8000.0))]);
        rows.push((key, [
            db(soft_share(&sb, 4000.0)),
            db(soft_share(&sr, 4000.0)),
            db(soft_share(&sr, 4000.0)) - db(soft_share(&sb, 4000.0)),
            db(band(&sr, 20.0, 2000.0)) - db(band(&sb, 20.0, 2000.0)),
            db(band(&sr, 4000.0, 8000.0)) - db(band(&sb, 4000.0, 8000.0)),
            db(gain(f1)),
            db(gain(string_partial(k.f1, k.b, 2))),
            db(high_gain),
        ]));
    }
    let dev = |x: &[f64], i: usize| -> f64 {
        let mut near: Vec<f64> = x[i.saturating_sub(2)..(i + 3).min(x.len())].to_vec();
        near.sort_by(f64::total_cmp);
        x[i] - near[near.len() / 2]
    };
    println!("== boardkeys v60: attack >4k (0-50 ms) bridge, radiated, board's change | board's change below 2k, 4-8k | board gain at partial 1, 2, 4-8k (dB); [deviation from the 5-key median] | raw gain at partial 1 left, right | level 0-0.5 s, its partial 1 | partial 1 left minus right | the board's accident at this key, and what the regulation evened (dB)");
    for i in 0..rows.len() {
        let d: Vec<String> = (0..8).map(|c| format!("{:+5.1}", dev(&rows.iter().map(|r| r.1[c]).collect::<Vec<_>>(), i))).collect();
        let v = rows[i].1;
        println!("{:>4} | {:+6.1} {:+6.1} {:+5.1} | {:+6.1} {:+6.1} | {:+6.1} {:+6.1} {:+6.1} | [{}] | {:+6.1} {:+6.1} | {:+6.1} {:+6.1} | {:+6.1} | {:+5.1} {:+5.1}", name(rows[i].0), v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7], d.join(" "), more[i][0], more[i][1], more[i][2], more[i][3], more[i][4], more[i][5], more[i][6]);
    }
    // the summary over C4-C7
    let inside: Vec<usize> = (0..rows.len()).filter(|&i| (60..=96).contains(&rows[i].0)).collect();
    if inside.len() < 3 {
        return;
    }
    let column = |c: usize| -> Vec<f64> { rows.iter().map(|r| r.1[c]).collect() };
    let stats = |x: &[f64]| -> (f64, f64, f64) {
        let spread = (inside.iter().map(|&i| dev(x, i).powi(2)).sum::<f64>() / inside.len() as f64).sqrt();
        let steps: Vec<f64> = inside.windows(2).map(|w| (x[w[1]] - x[w[0]]).abs()).collect();
        (spread, steps.iter().sum::<f64>() / steps.len() as f64, steps.iter().copied().fold(0.0, f64::max))
    };
    let (level, fundamental): (Vec<f64>, Vec<f64>) = (more.iter().map(|m| m[2]).collect(), more.iter().map(|m| m[3]).collect());
    let (lv, fu, at, hi) = (stats(&level), stats(&fundamental), stats(&column(1)), stats(&column(4)));
    let (el, er): (Vec<f64>, Vec<f64>) = (more.iter().map(|m| m[0]).collect(), more.iter().map(|m| m[1]).collect());
    let (dl, dr): (Vec<f64>, Vec<f64>) = (inside.iter().map(|&i| dev(&el, i)).collect(), inside.iter().map(|&i| dev(&er, i)).collect());
    let mean = |x: &[f64]| x.iter().sum::<f64>() / x.len() as f64;
    let (ml, mr) = (mean(&dl), mean(&dr));
    let cov = dl.iter().zip(&dr).map(|(a, b)| (a - ml) * (b - mr)).sum::<f64>();
    let corr = cov / (dl.iter().map(|a| (a - ml).powi(2)).sum::<f64>() * dr.iter().map(|b| (b - mr).powi(2)).sum::<f64>()).sqrt();
    let trend: Vec<String> = [(60u8, 71u8), (72, 83), (84, 95)]
        .iter()
        .map(|&(a, z)| {
            let keys: Vec<usize> = inside.iter().copied().filter(|&i| (a..=z).contains(&rows[i].0)).collect();
            format!("{}-{} {:+.1}", name(a), name(z), keys.iter().map(|&i| level[i]).sum::<f64>() / keys.len() as f64)
        })
        .collect();
    // the 4-8 kHz band as heard: each step beyond the bridge's own
    let excess: Vec<f64> = inside.windows(2).map(|w| (more[w[1]][7] - more[w[0]][7]).abs() - (more[w[1]][8] - more[w[0]][8]).abs()).collect();
    let side: Vec<f64> = more.iter().map(|m| m[4]).collect();
    let side_steps: Vec<f64> = inside.windows(2).map(|w| (side[w[1]] - side[w[0]]).abs()).collect();
    let corrected: Vec<String> = inside.iter().filter(|&&i| more[i][6] != 0.0).map(|&i| format!("{} {:+.1} (evened {:+.1})", name(rows[i].0), more[i][5], more[i][6])).collect();
    println!("== boardkeys summary C4-C7: level spread {:.1} mean step {:.1} worst {:.1} | partial 1 spread {:.1} mean step {:.1} worst {:.1} | attack share spread {:.1} worst step {:.1} | 4-8k change mean step {:.1} worst {:.1} | ears' partial-1 deviations correlate {corr:+.2} | level by register {} | 4-8k radiated steps beyond the bridge's: mean {:+.1} worst {:+.1} | partial 1 left-right: worst {:+.1}, worst step {:.1} | evened {} of {} keys: {}", lv.0, lv.1, lv.2, fu.0, fu.1, fu.2, at.0, at.2, hi.1, hi.2, trend.join(", "), excess.iter().sum::<f64>() / excess.len() as f64, excess.iter().copied().fold(f64::MIN, f64::max), side.iter().copied().fold(0.0, |a: f64, b| if b.abs() > a.abs() { b } else { a }), side_steps.iter().copied().fold(0.0, f64::max), corrected.len(), inside.len(), corrected.join(", "));
}

/// The touch by key, dry: each key's level over its first 0.5 s at
/// v20/40/67/110/127, and how much more or less it grows from v40 to v110
/// and from v20 to v127 than the touch law (the hammer's speed in dB);
/// then by register the median and the worst key. TOUCH_KEYS: a list.
fn section_touchkeys() {
    let mut p = stock(true);
    let keys: Vec<u8> = std::env::var("TOUCH_KEYS").ok().map_or((21u8..=108).collect(), |v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    let vels = [20u8, 40, 67, 110, 127];
    let law = |a: u8, b: u8| 20.0 * (makepad_piano_waveguide::hammer_speed(b) / makepad_piano_waveguide::hammer_speed(a)).log10();
    println!("== touchkeys at {:.0} Hz: key | growth v40-110, v20-127 over the law's ({:.1}, {:.1} dB) | v40-67, v67-110 over the law's | the key's touch span", fs(), law(40, 110), law(20, 127));
    if std::env::var("TOUCH_DIAG").is_ok() {
        // growth v40-110 over the law's by four measures: the output over
        // 0-0.5 s, the output through a 0.3 s Hann window, and the bridge
        // velocity the same two ways
        let hann = |x: &[f32]| -> f64 {
            let n = x.len();
            x.iter().enumerate().map(|(i, &v)| (v as f64 * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).powi(2)).sum()
        };
        for &key in &keys {
            let mut m = [[0.0; 5]; 2];
            for (i, vel) in [40u8, 110].into_iter().enumerate() {
                p.reset();
                let (l, r) = render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 0.6);
                let o = onset(&l, &r);
                let b = p.bridge_velocity(key, vel, (0.6 * fs()) as usize);
                // the bridge's spectrum through the board's exact gain at
                // each bin
                let sp = spectrum(sect(&b, 0.0, 0.3), sect(&b, 0.0, 0.3));
                let weighted: f64 = sp.1.iter().enumerate().skip(1).take_while(|(k, _)| *k as f64 * sp.0 < 0.4 * fs()).map(|(k, &pw)| {
                    let t = p.board_transfer(key, k as f64 * sp.0, false);
                    pw * 0.5 * (t[0].0 * t[0].0 + t[0].1 * t[0].1 + t[1].0 * t[1].0 + t[1].1 * t[1].1)
                }).sum();
                m[i] = [db(energy(sect(&l[o..], 0.0, 0.5), sect(&r[o..], 0.0, 0.5))), db(hann(sect(&l[o..], 0.0, 0.3)) + hann(sect(&r[o..], 0.0, 0.3))), db(energy(sect(&b, 0.0, 0.5), sect(&b, 0.0, 0.5))), db(hann(sect(&b, 0.0, 0.3))), db(weighted)];
            }
            let g: Vec<String> = (0..5).map(|j| format!("{:+5.1}", m[1][j] - m[0][j] - law(40, 110))).collect();
            println!("diag {:>4} | out 0.5 {} | out hann {} | bridge 0.5 {} | bridge hann {} | bridge x board {} | span {:.3}", name(key), g[0], g[1], g[2], g[3], g[4], p.touch(key));
        }
        return;
    }
    let mut rows: Vec<(u8, [f64; 4])> = Vec::new();
    for &key in &keys {
        let mut level = [0.0; 5];
        for (i, &vel) in vels.iter().enumerate() {
            p.reset();
            let (l, r) = render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 0.6);
            let o = onset(&l, &r);
            level[i] = db(energy(sect(&l[o..], 0.0, 0.5), sect(&r[o..], 0.0, 0.5)));
        }
        let row = [level[3] - level[1] - law(40, 110), level[4] - level[0] - law(20, 127), level[2] - level[1] - law(40, 67), level[3] - level[2] - law(67, 110)];
        println!("tk {:>4} | {:+5.1} {:+5.1} | {:+5.1} {:+5.1} | span {:.3}", name(key), row[0], row[1], row[2], row[3], p.touch(key));
        rows.push((key, row));
    }
    println!("== touchkeys by register: growth v40-110 and v20-127 over the law's, median [worst key]");
    let mut from = 21u8;
    while from <= 108 {
        let to = if from == 21 { 23 } else { from + 11 };
        let reg: Vec<&(u8, [f64; 4])> = rows.iter().filter(|r| (from..=to).contains(&r.0)).collect();
        if !reg.is_empty() {
            let col = |c: usize| {
                let mut v: Vec<f64> = reg.iter().map(|r| r.1[c]).collect();
                v.sort_by(|a, b| a.total_cmp(b));
                let worst = reg.iter().max_by(|a, b| a.1[c].abs().total_cmp(&b.1[c].abs())).unwrap();
                format!("{:+5.1} [{} {:+.1}]", v[v.len() / 2], name(worst.0), worst.1[c])
            };
            println!("reg {:>4}-{:<4} | {} | {}", name(from), name(to.min(108)), col(0), col(1));
        }
        from = to + 1;
    }
}

/// What sets each key's regulation gain from E6 up (`REG_FROM`..`REG_TO`):
/// the bridge velocity of a v67 blow over 0-0.3 s, partials 1-6 (dB), the
/// bridge's resistance and the prompt decay, the board's raw gain at
/// partial 1 and the key's regulation gain (dB).
fn section_regkeys() {
    let p = stock(true);
    let from: u8 = std::env::var("REG_FROM").ok().and_then(|x| x.parse().ok()).unwrap_or(88);
    let to: u8 = std::env::var("REG_TO").ok().and_then(|x| x.parse().ok()).unwrap_or(100);
    let mut q = stock(true);
    let felt = q.felt_at_mm();
    println!("== regkeys: key | bridge partials 1-6 at v67, 0-0.3 s (dB) | bridge R kg/s, strings, losses b1 b3, damper | board raw gain at p1 dB | regulation gain dB");
    for key in from..=to {
        let k = p.scale()[(key - 21) as usize].clone();
        q.reset();
        render(&mut q, &[(0.0, PianoEvent::NoteOn { key, velocity: 67 })], 0.02);
        let (contact, force) = q.last_blow(key);
        let v = q.bridge_velocity(key, 67, (0.3 * fs()) as usize);
        let s = spectrum(&v, &v);
        let parts: Vec<String> = (1..=6).map(|n| format!("{:6.1}", db(partial_power(&s, k.f1, k.b, n)))).collect();
        let f1 = string_partial(k.f1, k.b, 1);
        let t = p.board_transfer(key, f1, false);
        let raw = 0.5 * (t[0].0 * t[0].0 + t[0].1 * t[0].1 + t[1].0 * t[1].0 + t[1].1 * t[1].1);
        let (l, r) = p.board_gain(key, f1);
        println!("{:>4} | {} | R {:7.0} x{} b1 {:.2} b3 {:.1e} damper {:.2} | felt {:6.1} N, v67 contact {:.3} ms (asked {:.3}) peak {:5.1} N | {:+6.1} | {:+6.1}", name(key), parts.join(" "), k.bridge_r, k.strings, k.b1, k.b3, k.damper_t60, felt[(key - 21) as usize], contact * 1e3, k.contact * 1e3, force, db(raw), db(0.5 * (l + r)) - db(raw));
    }
}

/// Power spectrum without a window (for impulse responses).
fn spectrum_flat(l: &[f32], r: &[f32]) -> (f64, Vec<f64>) {
    let n = l.len().next_power_of_two();
    let mut power = vec![0.0; n / 2 + 1];
    for ch in [l, r] {
        let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
        for (i, &v) in ch.iter().enumerate() {
            re[i] = v as f64;
        }
        fft(&mut re, &mut im);
        for k in 0..=n / 2 {
            power[k] += 0.5 * (re[k] * re[k] + im[k] * im[k]);
        }
    }
    (fs() / n as f64, power)
}

fn section_halo() {
    println!("== halo v68: share in dB of a held note's sound at 0.3-1.5 s that lies off its own partials (more than 0.35 f1 from each); the same with the pedal down; level of 1-2 s with the pedal re without");
    let mut p = stock(true);
    for key in [36u8, 48, 60, 67, 72, 84] {
        let k = p.scale()[(key - 21) as usize].clone();
        let mut line = String::new();
        let mut late = [0.0; 2];
        for (i, pedal) in [0.0f32, 1.0].into_iter().enumerate() {
            p.reset();
            let (l, r) = render(&mut p, &[(0.0, PianoEvent::Sustain { value: pedal }), (0.0, PianoEvent::NoteOn { key, velocity: 68 })], 2.1);
            let s = spectrum(sect(&l, 0.3, 1.5), sect(&r, 0.3, 1.5));
            let total = band(&s, 20.0, 16000.0);
            let mut on = 0.0;
            for n in 1..=200 {
                let f = string_partial(k.f1, k.b, n);
                if f > 16000.0 {
                    break;
                }
                on += band(&s, f - 0.35 * k.f1, f + 0.35 * k.f1);
            }
            late[i] = energy(sect(&l, 1.0, 2.0), sect(&r, 1.0, 2.0));
            line += &format!(" {:6.1}", db(((total - on) / total).max(1e-12)));
        }
        println!("{:>3} off-series{line} | pedal lifts 1-2 s by {:+.1} dB", name(key), db(late[1] / late[0]));
    }
}

fn section_stereo() {
    println!("== stereo v68, every key at 0.05-1.2 s: per register the mean correlation of left and right, the mean of left minus right level, its spread from key to key (rms) and the largest, dB");
    let mut rows: Vec<(f64, f64)> = Vec::new();
    for key in 21u8..=108 {
        let (l, r) = note(key, 68, 1.25, true);
        let (l, r) = (sect(&l, 0.05, 1.2), sect(&r, 0.05, 1.2));
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum::<f64>();
        rows.push((dot(l, r) / (dot(l, l) * dot(r, r)).sqrt(), db(dot(l, l) / dot(r, r))));
    }
    for (label, a, z) in [("A0..B2", 0usize, 27usize), ("C3..B4", 27, 51), ("C5..B6", 51, 75), ("C7..C8", 75, 88)] {
        let n = (z - a) as f64;
        let corr = rows[a..z].iter().map(|r| r.0).sum::<f64>() / n;
        let lowest = rows[a..z].iter().map(|r| r.0).fold(1.0f64, f64::min);
        let mean = rows[a..z].iter().map(|r| r.1).sum::<f64>() / n;
        let spread = (rows[a..z].iter().map(|r| (r.1 - mean).powi(2)).sum::<f64>() / n).sqrt();
        let worst = rows[a..z].iter().map(|r| (r.1 - mean).abs()).fold(0.0f64, f64::max);
        println!("{label}: correlation {corr:+.2} (lowest {lowest:+.2}) | left-right {mean:+.1} dB, spread {spread:.1}, largest {worst:.1}");
    }
}

fn section_release() {
    println!("== release v68: a key held 0.5 s, then let go: level dB re the 50 ms before the release at 0-50 ms, 50-100 ms, 100-200 ms, 200-400 ms after; then the same for the band above 5 f1 only (high partials go first under a damper)");
    let mut p = stock(true);
    p.set_duplex(0.0);
    p.set_coupling(0.0);
    for key in [24u8, 33, 40, 48, 60, 72, 84] {
        p.reset();
        let f1 = p.scale()[(key - 21) as usize].f1;
        let (l, r) = render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: 68 }), (0.5, PianoEvent::NoteOff { key })], 1.0);
        let level = |a: f64, b: f64, lo: f64| {
            let s = spectrum(sect(&l, a, b), sect(&r, a, b));
            band(&s, lo, 20000.0)
        };
        let mut line = String::new();
        for lo in [20.0, 5.0 * f1] {
            let before = level(0.45, 0.5, lo);
            for (a, b) in [(0.5, 0.55), (0.55, 0.6), (0.6, 0.7), (0.7, 0.9)] {
                line += &format!(" {:6.1}", db(level(a, b, lo) / before));
            }
            line += " |";
        }
        println!("{:>3}{line}", name(key));
    }
}

fn section_onset() {
    println!("== onset v68: share of the first 50 ms' energy that falls in its first 5 ms (dry, with the room, the recording), then ms from onset to within 3 dB of the peak of the 1 ms envelope (dry, room), and the level dB of 5-10, 10-20, 20-50 ms re 0-5 ms (dry)");
    for line in FIXTURE.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let (key, vel): (u8, u8) = (f[0].parse().unwrap(), f[1].parse().unwrap());
        if vel != 68 {
            continue;
        }
        let reference: f64 = f[17].parse().unwrap();
        let mut cols = String::new();
        let mut rise = String::new();
        let mut steps = String::new();
        for dry in [true, false] {
            let (l, r) = note(key, vel, 0.6, dry);
            let o = onset(&l, &r);
            let (l, r) = (&l[o..], &r[o..]);
            let share = energy(sect(l, 0.0, 0.005), sect(r, 0.0, 0.005)) / energy(sect(l, 0.0, 0.05), sect(r, 0.0, 0.05));
            cols += &format!(" {share:.4}");
            let env: Vec<f64> = (0..400).map(|i| energy(&l[i * 48..(i + 1) * 48], &r[i * 48..(i + 1) * 48])).collect();
            let peak = env.iter().copied().fold(0.0, f64::max);
            rise += &format!(" {:4}", env.iter().position(|&e| e > 0.5 * peak).unwrap());
            if dry {
                let first = energy(sect(l, 0.0, 0.005), sect(r, 0.0, 0.005)) / 0.005;
                for (a, b) in [(0.005, 0.01), (0.01, 0.02), (0.02, 0.05)] {
                    steps += &format!(" {:+5.1}", db(energy(sect(l, a, b), sect(r, a, b)) / (b - a) / first));
                }
            }
        }
        println!("{:>3} share{cols} {reference:.4} | rise ms{rise} | then{steps}", name(key));
    }
}

/// One piano for the scoreboard: either engine behind the same few calls.
enum Player {
    Waveguide(WaveguidePiano),
    Modal(Box<makepad_piano_model::Piano>),
}

#[derive(Clone, Copy)]
enum Play {
    On(u8, u8),
    Off(u8),
    Pedal(f32),
}

impl Play {
    fn waveguide(self) -> PianoEvent {
        match self {
            Play::On(key, velocity) => PianoEvent::NoteOn { key, velocity },
            Play::Off(key) => PianoEvent::NoteOff { key },
            Play::Pedal(value) => PianoEvent::Sustain { value },
        }
    }
}

impl Player {
    fn both(dry: bool) -> [(&'static str, Player); 2] {
        let preset = makepad_piano_model::PIANO_PRESETS.iter().find(|p| p.is_default).unwrap();
        let mut modal = makepad_piano_model::Piano::new_with_preset(fs() as f32, preset);
        if dry {
            modal.set_reverb_mix(0.0);
            modal.set_early_reflection_level(0.0);
            modal.set_soft_clip(false);
        }
        [("Waveguide Grand", Player::Waveguide(stock(dry))), ("Concert Grand", Player::Modal(Box::new(modal)))]
    }

    /// (first partial Hz, inharmonicity) of a key.
    fn string(&self, key: u8) -> (f64, f64) {
        match self {
            Player::Waveguide(p) => (p.scale()[(key - 21) as usize].f1, p.scale()[(key - 21) as usize].b),
            Player::Modal(p) => {
                let k = p.key_info(key).unwrap();
                (k.f0 as f64 * (1.0 + k.b_coeff as f64).sqrt(), k.b_coeff as f64)
            }
        }
    }

    /// From rest: (seconds, event) script rendered in 256-sample blocks.
    fn play(&mut self, script: &[(f64, Play)], secs: f64) -> (Vec<f32>, Vec<f32>) {
        let total = (secs * fs()) as usize;
        let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
        match self {
            Player::Waveguide(p) => p.reset(),
            Player::Modal(p) => makepad_piano_model::Instrument::reset(&mut **p),
        }
        let mut pos = 0;
        while pos < total {
            let n = 256.min(total - pos);
            let due = script.iter().filter_map(|&(t, e)| {
                let at = (t * fs()).round() as usize;
                (at >= pos && at < pos + n).then_some(((at - pos) as u32, e))
            });
            match self {
                Player::Waveguide(p) => {
                    let events: Vec<TimedEvent> = due.map(|(offset, e)| TimedEvent { offset, event: e.waveguide() }).collect();
                    p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
                }
                Player::Modal(p) => {
                    use makepad_piano_model::{PianoEvent as E, TimedEvent as T};
                    let events: Vec<T> = due
                        .map(|(offset, e)| T { offset, event: match e { Play::On(key, velocity) => E::NoteOn { key, velocity }, Play::Off(key) => E::NoteOff { key }, Play::Pedal(value) => E::Sustain { value } } })
                        .collect();
                    p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
                }
            }
            pos += n;
        }
        (l, r)
    }
}

/// Both pianos on one table, measured in this process and so on one box.
fn section_scoreboard() {
    println!("== scoreboard: both pianos, dry unless said, measured in one process");
    let rows: Vec<(u8, u8, [f64; 11])> = FIXTURE.lines().filter(|l| !l.starts_with('#')).skip(1).map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        (f[0].parse().unwrap(), f[1].parse().unwrap(), std::array::from_fn(|i| f[10 + i].parse().unwrap()))
    }).collect();
    let columns = ["mid share early", "high share early", "mid share late", "high share late", "partial 1 early", "partial 1 late", "register level", "onset 5/50 ms", "decay 20-500", "decay 0.5-2k", "decay 2-8k"];
    let mut table: Vec<Vec<String>> = Vec::new();
    for (label, mut piano) in Player::both(true) {
        let mut out = vec![label.to_string()];
        // 1. the fixture's eleven columns, mean |model - recording| by register
        let model: Vec<[f64; 11]> = rows.iter().map(|r| { let (l, rr) = piano.play(&[(0.0, Play::On(r.0, r.1))], 2.6); fixture_metrics(&l, &rr, r.0) }).collect();
        let mut sum = [[0.0f64; 11]; 2];
        let mut count = [0.0f64; 2];
        for (i, row) in rows.iter().enumerate() {
            let c4 = rows.iter().position(|r| r.0 == 60 && r.1 == row.1).unwrap();
            let g = if row.0 <= 48 { 0 } else { 1 };
            count[g] += 1.0;
            for k in 0..11 {
                let m = if k == 6 { 20.0 * (model[i][6] / model[c4][6]).log10() } else { model[i][k] };
                sum[g][k] += (m - row.2[k]).abs();
            }
        }
        for k in 0..11 {
            out.push(format!("{:.2} / {:.2}", sum[0][k] / count[0], sum[1][k] / count[1]));
        }
        // 2. ripple: how far partials 1-4 of a held mid note stray from a
        // smooth decay (dB rms about a quadratic fit of level over 0.3-2.5 s)
        let mut ripple = Vec::new();
        // 3. halo: share of a held note's sound at 0.3-1.5 s off its own partials
        let mut halo = Vec::new();
        // 4. pedal lift: level of 1-2 s with the pedal down re without
        let mut lift = Vec::new();
        for key in [48u8, 55, 60, 67, 72] {
            let (f1, b) = piano.string(key);
            let (l, r) = piano.play(&[(0.0, Play::On(key, 68))], 2.7);
            for n in 1..=4usize {
                let f = string_partial(f1, b, n);
                let times: Vec<f64> = (0..43).map(|i| 0.3 + 0.05 * i as f64).collect();
                let level: Vec<f64> = times.iter().map(|&t| { let s = spectrum(sect(&l, t, t + 0.1), sect(&r, t, t + 0.1)); db(band(&s, f - 0.4 * f1, f + 0.4 * f1)) }).collect();
                ripple.push(quadratic_residual(&times, &level));
            }
            let s = spectrum(sect(&l, 0.3, 1.5), sect(&r, 0.3, 1.5));
            let total = band(&s, 20.0, 16000.0);
            let mut on = 0.0;
            for n in 1..=200 {
                let f = string_partial(f1, b, n);
                if f > 16000.0 {
                    break;
                }
                on += band(&s, f - 0.35 * f1, f + 0.35 * f1);
            }
            halo.push(db(((total - on) / total).max(1e-12)));
            let (pl, pr) = piano.play(&[(0.0, Play::Pedal(1.0)), (0.0, Play::On(key, 68))], 2.1);
            lift.push(db(energy(sect(&pl, 1.0, 2.0), sect(&pr, 1.0, 2.0)) / energy(sect(&l, 1.0, 2.0), sect(&r, 1.0, 2.0))));
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        out.push(format!("{:.2} (max {:.2})", mean(&ripple), ripple.iter().copied().fold(0.0, f64::max)));
        out.push(format!("{:.1} ({:.1} .. {:.1})", mean(&halo), halo.iter().copied().fold(f64::MAX, f64::min), halo.iter().copied().fold(f64::MIN, f64::max)));
        out.push(format!("{:+.1} ({:+.1} .. {:+.1})", mean(&lift), lift.iter().copied().fold(f64::MAX, f64::min), lift.iter().copied().fold(f64::MIN, f64::max)));
        table.push(out);
    }
    // 5. CPU: the pedalled 48-note passage with each piano's room on, the
    // two timed alternately, best of 3 each
    let mut script = vec![(0.0, Play::Pedal(1.0))];
    for i in 0..48u32 {
        script.push((0.125 * i as f64, Play::On(24 + ((i * 29) % 72) as u8, 60 + ((i * 13) % 50) as u8)));
    }
    let mut pianos = Player::both(false);
    let mut best = [f64::MAX; 2];
    for _ in 0..3 {
        for (i, (_, piano)) in pianos.iter_mut().enumerate() {
            let t = std::time::Instant::now();
            std::hint::black_box(piano.play(&script, 10.0));
            best[i] = best[i].min(t.elapsed().as_secs_f64() / 10.0);
        }
    }
    for (row, cpu) in table.iter_mut().zip(best) {
        row.push(format!("{cpu:.3} s/s"));
    }
    let names: Vec<String> = columns.iter().map(|c| format!("{c}, mean |err| A0..C3 / C4..C7")).chain([
        "ripple of partials 1-4, C3..C5, dB rms".to_string(),
        "halo: off-series share of a held note, dB".to_string(),
        "pedal lift at 1-2 s, dB".to_string(),
        "CPU, pedalled 48-note passage".to_string(),
    ]).collect();
    println!("{:<52} {:>24} {:>24}", "", table[0][0], table[1][0]);
    for (i, name) in names.iter().enumerate() {
        println!("{name:<52} {:>24} {:>24}", table[0][i + 1], table[1][i + 1]);
    }
}

fn section_ripple() {
    println!("== ripple v68: dB rms of partials 1-4 about a smooth (quadratic) decay over 0.3-2.5 s, per key; then C4's partials 1 and 2 every 0.2 s from 0.1 s, dB");
    let p = stock(true);
    let mut all = Vec::new();
    for key in [48u8, 55, 60, 67, 72] {
        let k = p.scale()[(key - 21) as usize].clone();
        let (l, r) = note(key, 68, 2.7, true);
        let level = |n: usize, t: f64| {
            let f = string_partial(k.f1, k.b, n);
            let s = spectrum(sect(&l, t, t + 0.1), sect(&r, t, t + 0.1));
            db(band(&s, f - 0.4 * k.f1, f + 0.4 * k.f1))
        };
        let times: Vec<f64> = (0..43).map(|i| 0.3 + 0.05 * i as f64).collect();
        let per: Vec<f64> = (1..=4).map(|n| quadratic_residual(&times, &times.iter().map(|&t| level(n, t)).collect::<Vec<_>>())).collect();
        println!("{:>3} {}", name(key), per.iter().map(|x| format!("{x:5.2}")).collect::<Vec<_>>().join(" "));
        all.extend(per);
        if key == 60 {
            for n in 1..=2 {
                println!("    C4 partial {n}: {}", (0..13).map(|i| format!("{:6.1}", level(n, 0.1 + 0.2 * i as f64))).collect::<Vec<_>>().join(""));
            }
        }
    }
    println!("mean {:.2} dB", all.iter().sum::<f64>() / all.len() as f64);
}

/// RMS of `y` about its least-squares quadratic in `x`.
fn quadratic_residual(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let t: Vec<f64> = x.iter().map(|v| v - mean).collect();
    let (s2, s4) = (t.iter().map(|v| v * v).sum::<f64>(), t.iter().map(|v| v.powi(4)).sum::<f64>());
    let (sy, sty, st2y) = (y.iter().sum::<f64>(), t.iter().zip(y).map(|(a, b)| a * b).sum::<f64>(), t.iter().zip(y).map(|(a, b)| a * a * b).sum::<f64>());
    // y = a + b t + c t^2 with sum t = 0 = sum t^3 (the times are evenly spaced)
    let c = (n * st2y - s2 * sy) / (n * s4 - s2 * s2);
    let a = (sy - c * s2) / n;
    let b = sty / s2;
    (t.iter().zip(y).map(|(t, y)| (y - a - b * t - c * t * t).powi(2)).sum::<f64>() / n).sqrt()
}

fn section_cpu() {
    // The passage makepad-piano-model's probe times: sustain down, 48 notes
    // across the compass over 6 s, rung for 10 s in all; `span=85` takes
    // it up to the top key.
    let span = arg("span").unwrap_or(72.0) as u32;
    let mut script = vec![(0.0, PianoEvent::Sustain { value: 1.0 })];
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % span) as u8;
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


/// Third-octave band centres, 25 Hz to 16 kHz.
fn third_octaves() -> Vec<f64> {
    (0..29).map(|i| 25.0 * 2f64.powf(i as f64 / 3.0)).collect()
}

/// Long-term average power spectra of a stereo signal: the mean of the two
/// ears, the mono sum (L+R)/2 and the side (L-R)/2, per bin (Hann frames of
/// 8192, half overlap). Returns the bin width and the three spectra.
fn ltas(l: &[f32], r: &[f32]) -> (f64, [Vec<f64>; 3]) {
    let n = 8192;
    let w: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos()).collect();
    let mut out: [Vec<f64>; 3] = std::array::from_fn(|_| vec![0.0; n / 2 + 1]);
    let mut start = 0;
    while start + n <= l.len() {
        let mut ch = [(vec![0.0; n], vec![0.0; n]), (vec![0.0; n], vec![0.0; n])];
        for (c, x) in ch.iter_mut().zip([l, r]) {
            for i in 0..n {
                c.0[i] = x[start + i] as f64 * w[i];
            }
            fft(&mut c.0, &mut c.1);
        }
        let [(lr, li), (rr, ri)] = &ch;
        for k in 0..=n / 2 {
            out[0][k] += 0.5 * (lr[k] * lr[k] + li[k] * li[k] + rr[k] * rr[k] + ri[k] * ri[k]);
            out[1][k] += 0.25 * ((lr[k] + rr[k]).powi(2) + (li[k] + ri[k]).powi(2));
            out[2][k] += 0.25 * ((lr[k] - rr[k]).powi(2) + (li[k] - ri[k]).powi(2));
        }
        start += n / 2;
    }
    (fs() / n as f64, out)
}

/// Power per third-octave band of a spectrum.
fn octave_bands(bin: f64, p: &[f64]) -> Vec<f64> {
    let s = (bin, p.to_vec());
    third_octaves().iter().map(|&c| band(&s, c / 2f64.powf(1.0 / 6.0), c * 2f64.powf(1.0 / 6.0))).collect()
}

fn row(v: &[f64]) -> String {
    v.iter().map(|x| format!("{x:6.1}")).collect::<Vec<_>>().join("")
}

/// The switches of the colour hunt. Each changes one thing against the
/// instrument as it is built.
const SWITCHES: [(&str, &str); 10] = [
    ("phantoms-off", "no longitudinal (phantom) components"),
    ("phantoms-resonant", "phantoms through their two resonators only, without the squared motion going straight out"),
    ("width-0", "one listening point instead of two 0.4 m apart (mono instrument)"),
    ("ear-phase-0", "two listening points, level differences only: every mode reaches both with the same timing"),
    ("mono", "the stereo output summed to one channel, (L+R)/2 on both"),
    ("room-off", "no room"),
    ("limiter-off", "no output limiter"),
    ("board-direct", "each mode's shape evaluated at the key's own place on the bridge, not interpolated between 4 places"),
    ("board-8", "mode shapes sampled at 8 places along the bridge instead of 4"),
    ("duplex-off", "duplex segments muted"),
];

/// The instrument with one switch thrown (`stock` for none). Not dry: the
/// room is `room=` or the instrument's own.
fn variant(name: &str, dry: bool) -> WaveguidePiano {
    let (mut v, mut b) = settings();
    match name {
        "phantoms-off" => v.phantoms = 0.0,
        "phantoms-resonant" => v.phantom_direct = 0.0,
        "width-0" => b.width = 0.0,
        "ear-phase-0" => b.ear_phase = 0.0,
        "board-direct" => b.positions = 88,
        "board-8" => b.positions = 8,
        "duplex-off" => b.duplex = 0.0,
        "loss-high" => v.string_loss_high = LOSS_HIGH,
        "yield-1" => v.bridge_yield = 1.0,
        "felt-uncapped" => v.felt_ceiling = f64::INFINITY,
        "felt-cap-1.5" => v.felt_ceiling = 1.5,
        "direct-0" => {
            b.direct_gain = 0.0;
            b.floor = 0.0;
        }
        "sideways-0" => v.sideways = 0.0,
        "unison-0" => v.unison_cents = 0.0,
        "even-0" => v.strike_unevenness = 0.0,
        "bass-combined" => {
            v.phantoms = 0.0;
            v.string_loss_high = LOSS_HIGH;
            v.bridge_yield = 1.0;
        }
        _ => {}
    }
    let mut p = build(&v, &b, dry);
    match name {
        "room-off" => p.set_room_mix(0.0),
        "limiter-off" => p.set_limit_knee(1.0),
        "coupling-0" => p.set_coupling(0.0),
        "knock-0" => p.set_knock(0.0),
        "damper-slow" => p.set_damper_slew(10.0),
        _ => {}
    }
    p
}

/// The strings' high-frequency loss in the `loss-high` switch, times as
/// built.
const LOSS_HIGH: f64 = 10.0;

/// What each switch beyond `SWITCHES` does.
const MORE_SWITCHES: [(&str, &str); 8] = [
    ("loss-high", "the strings' high-frequency loss (b3) 10x: upper partials die sooner"),
    ("yield-1", "the bridge yields to fast motion as to slow (bridge_yield 1): a low note's upper partials drain as fast as its fundamental"),
    ("sideways-0", "no sideways string motion"),
    ("bass-combined", "phantoms off, high-frequency loss 10x and bridge_yield 1 together"),
    ("aligned", "event times moved down to a multiple of 32 samples, so every 32-sample block is full and in step"),
    ("coupling-0", "no sympathetic feedback between keys"),
    ("damper-slow", "every damper takes 10x as long to settle"),
    ("knock-0", "no case knock"),
];

/// (seconds, event) in host buffers of `block` samples.
fn render_in(p: &mut WaveguidePiano, script: &[(f64, Play)], secs: f64, block: usize) -> (Vec<f32>, Vec<f32>) {
    let total = (secs * fs()) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    p.reset();
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let events: Vec<TimedEvent> = script
            .iter()
            .filter_map(|&(t, e)| {
                let at = (t * fs()).round() as usize;
                (at >= pos && at < pos + n).then_some(TimedEvent { offset: (at - pos) as u32, event: e.waveguide() })
            })
            .collect();
        p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

/// 48 notes across the compass, as the CPU passage, without the pedal: each
/// released after 0.3 s.
fn passage_run() -> (Vec<(f64, Play)>, f64) {
    let mut script = Vec::new();
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % 72) as u8;
        script.push((0.125 * i as f64, Play::On(key, 60 + ((i * 13) % 50) as u8)));
        script.push((0.125 * i as f64 + 0.3, Play::Off(key)));
    }
    (script, 7.5)
}

/// C, Am, F, G between C3 and C5, a second each, four at v50, four at v80,
/// four at v110.
fn passage_chords() -> (Vec<(f64, Play)>, f64) {
    let chords: [&[u8]; 4] = [&[48, 52, 55, 60, 64, 67, 72], &[48, 52, 57, 60, 64, 69, 72], &[48, 53, 57, 60, 65, 69, 72], &[50, 55, 59, 62, 67, 71]];
    let mut script = Vec::new();
    for (i, vel) in [50u8, 80, 110].into_iter().enumerate() {
        for (j, chord) in chords.iter().enumerate() {
            let t = (4 * i + j) as f64;
            for &k in chord.iter() {
                script.push((t, Play::On(k, vel)));
                script.push((t + 0.95, Play::Off(k)));
            }
        }
    }
    (script, 13.5)
}

/// A soft middle C, held 3 s.
fn passage_soft_note() -> (Vec<(f64, Play)>, f64) {
    (vec![(0.05, Play::On(60, 40)), (3.05, Play::Off(60))], 3.6)
}

/// A forte C major chord, C3 to C5, held 3 s.
fn passage_forte_chord() -> (Vec<(f64, Play)>, f64) {
    let mut script = Vec::new();
    for k in [48u8, 52, 55, 60, 64, 67, 72] {
        script.push((0.05, Play::On(k, 105)));
        script.push((3.05, Play::Off(k)));
    }
    (script, 3.8)
}

/// Four bars, C Am F G, broken chords in the left hand and a melody in the
/// right, the pedal changed at each bar, then a held C chord: velocities
/// as Score plays a written or a played phrase (60-90).
fn passage_phrase() -> (Vec<(f64, Play)>, f64) {
    let beat = 0.5;
    let bars: [(u8, bool, [u8; 4]); 4] = [(36, true, [76, 74, 72, 67]), (33, false, [69, 72, 76, 74]), (29, true, [72, 69, 65, 69]), (31, true, [71, 74, 79, 77])];
    let mut script = Vec::new();
    for (b, &(root, major, melody)) in bars.iter().enumerate() {
        let t0 = 0.1 + 4.0 * beat * b as f64;
        // the pedal comes up at the bar and goes down just after it
        if b > 0 {
            script.push((t0 - 0.01, Play::Pedal(0.0)));
        }
        script.push((t0 + 0.08, Play::Pedal(1.0)));
        let third = if major { 16 } else { 15 };
        for (i, step) in [0u8, 7, 12, third, 19, third, 12, 7].into_iter().enumerate() {
            let t = t0 + 0.5 * beat * i as f64;
            script.push((t, Play::On(root + step, if i == 0 { 70 } else { 58 + (i as u8 * 3) % 9 })));
            script.push((t + 0.45 * beat, Play::Off(root + step)));
        }
        for (i, &m) in melody.iter().enumerate() {
            let t = t0 + beat * i as f64;
            script.push((t, Play::On(m, 80 + (i as u8 * 5) % 10)));
            script.push((t + 0.96 * beat, Play::Off(m)));
        }
    }
    let t0 = 0.1 + 16.0 * beat;
    script.push((t0 - 0.01, Play::Pedal(0.0)));
    script.push((t0 + 0.08, Play::Pedal(1.0)));
    for (k, v) in [(24u8, 72u8), (36, 70), (67, 82), (72, 84), (76, 88)] {
        script.push((t0, Play::On(k, v)));
        script.push((t0 + 3.0, Play::Off(k)));
    }
    (script, t0 + 4.5)
}

/// What the output does to the limiter: largest sample and the share of
/// samples above its knee, with the limiter out of the way.
fn limiter_load(l: &[f32], r: &[f32]) -> (f64, f64) {
    let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs()));
    let over = l.iter().chain(r.iter()).filter(|x| x.abs() > 0.7).count();
    (peak as f64, over as f64 / (l.len() + r.len()) as f64)
}

/// Loud passages as Score plays them (the room as built, master 1): what
/// reaches the limiter and what it does. Without the limiter: the peak, the
/// level (rms over the passage) and the share of samples above 0.7 and above
/// -1 dBFS; with it: the peak out, the largest drop of the level of any 10 ms
/// against the unlimited render, and what of the limited output is not the
/// unlimited one times a gain (fitted per 10 ms): over the passage and in
/// its worst 10 ms, dB re the unlimited output there. A limiter that turns
/// the sound down leaves little; one that bends each sample leaves its
/// distortion. With AB_DIR set, each limited render goes there as
/// `<passage>-<AB_TAG>.wav`, at a K-weighted loudness of -23 dB.
fn section_limits() {
    let held = |keys: &[u8], vel: u8, pedal: bool| -> (Vec<(f64, Play)>, f64) {
        let mut script = Vec::new();
        if pedal {
            script.push((0.0, Play::Pedal(1.0)));
        }
        for &k in keys {
            script.push((0.05, Play::On(k, vel)));
            script.push((2.55, Play::Off(k)));
        }
        (script, 3.0)
    };
    let arpeggio = |vel: u8| -> (Vec<(f64, Play)>, f64) {
        let mut script = vec![(0.0, Play::Pedal(1.0))];
        for (i, k) in (0..16u8).map(|i| 36 + 12 * (i / 4) + [0, 4, 7, 12][(i % 4) as usize]).enumerate() {
            script.push((0.05 + 0.08 * i as f64, Play::On(k, vel)));
            script.push((2.55, Play::Off(k)));
        }
        (script, 3.0)
    };
    let run = |vel: u8| -> (Vec<(f64, Play)>, f64) {
        let mut script = Vec::new();
        for (i, k) in [72u8, 74, 76, 77, 79, 81, 83, 84, 86, 88, 89, 91, 93, 95, 96, 98, 100, 101, 103, 105, 107, 108].into_iter().enumerate() {
            script.push((0.05 + 0.1 * i as f64, Play::On(k, vel)));
            script.push((0.05 + 0.1 * i as f64 + 0.2, Play::Off(k)));
        }
        (script, 2.8)
    };
    let mid = [48u8, 52, 55, 60, 64, 67, 72];
    let treble = [72u8, 76, 79, 84, 88, 91, 96];
    let hands = [36u8, 43, 48, 52, 55, 60, 64, 67, 72, 76];
    let passages: Vec<(String, (Vec<(f64, Play)>, f64))> = vec![
        ("phrase".into(), passage_phrase()),
        ("chord C3-C5 v80".into(), held(&mid, 80, false)),
        ("chord C3-C5 v110".into(), held(&mid, 110, false)),
        ("chord C5-C7 v80".into(), held(&treble, 80, false)),
        ("chord C5-C7 v110".into(), held(&treble, 110, false)),
        ("hands C2-E5 v110 ped".into(), held(&hands, 110, true)),
        ("hands C2-E5 v127 ped".into(), held(&hands, 127, true)),
        ("arpeggio C2-C6 v110".into(), arpeggio(110)),
        ("run C5-C8 v110".into(), run(110)),
        ("octaves A0-A2 v127 ped".into(), held(&[21, 33, 45], 127, true)),
    ];
    // M_SERIES=<label>: that passage's bend per 10 ms over its first 0.3 s
    let series = std::env::var("M_SERIES").ok();
    let mut free = variant("limiter-off", false);
    let mut limited = stock(false);
    let ceiling = 10f64.powf(-1.0 / 20.0);
    println!("== limits at {:.0} Hz, as Score plays (room, master 1)", fs());
    println!("{:<22}| no limiter: peak dBFS, rms dB, above 0.7 %, above -1 dBFS % | limited: peak dBFS, worst 10 ms drop dB, bend dB, worst 10 ms bend dB", "");
    for (label, (script, secs)) in &passages {
        let (l, r) = render_in(&mut free, script, *secs, 256);
        let (ll, lr) = render_in(&mut limited, script, *secs, 256);
        let all = || l.iter().chain(r.iter()).map(|&x| x as f64);
        let peak = all().fold(0.0f64, |m, x| m.max(x.abs()));
        let n = (l.len() + r.len()) as f64;
        let over7 = all().filter(|x| x.abs() > 0.7).count() as f64 / n;
        let over1 = all().filter(|x| x.abs() > ceiling).count() as f64 / n;
        let lpeak = ll.iter().chain(lr.iter()).fold(0.0f32, |m, x| m.max(x.abs())) as f64;
        let w = (0.01 * fs()) as usize;
        let (mut drop, mut worst, mut bent) = (0.0f64, -300.0f64, 0.0f64);
        for j in (0..l.len().saturating_sub(w)).step_by(w) {
            let a = energy(&l[j..j + w], &r[j..j + w]);
            let pairs = || l[j..j + w].iter().zip(&ll[j..j + w]).chain(r[j..j + w].iter().zip(&lr[j..j + w])).map(|(&f, &g)| (f as f64, g as f64));
            let gain = pairs().map(|(f, g)| f * g).sum::<f64>() / pairs().map(|(f, _)| f * f).sum::<f64>().max(1e-30);
            let rest: f64 = pairs().map(|(f, g)| (g - gain * f).powi(2)).sum::<f64>() * 0.5;
            bent += rest;
            if a > 1e-6 {
                drop = drop.max(db(a / energy(&ll[j..j + w], &lr[j..j + w]).max(1e-30)));
                worst = worst.max(db(rest / a));
            }
            if series.as_deref() == Some(label.replace(' ', "_").as_str()) && j < (0.3 * fs()) as usize {
                print!(" {:.0}", db(rest / a.max(1e-30)));
            }
        }
        if let Some(dir) = std::env::var_os("AB_DIR") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).expect("AB_DIR");
            let g = (10f64.powf(-2.3) / k_loudness(&ll, &lr)).sqrt() as f32;
            let (al, ar): (Vec<f32>, Vec<f32>) = (ll.iter().map(|x| x * g).collect(), lr.iter().map(|x| x * g).collect());
            let file = format!("{}-{}.wav", label.replace(' ', "-"), std::env::var("AB_TAG").unwrap_or("ours".into()));
            write_wav(&dir.join(&file), &al, &ar);
            let top = al.iter().chain(ar.iter()).fold(0.0f32, |m, x| m.max(x.abs())) as f64;
            println!("{file} gain {:+.1} dB, peak {:+.1} dBFS", db((g * g) as f64), db(top * top));
        }
        println!("{label:<22}| {:+6.1} {:6.1} {:7.3} {:7.3} | {:+6.1} {drop:5.2} {:6.1} {worst:6.1}", db(peak * peak), db(energy(&l, &r) / l.len() as f64), 100.0 * over7, 100.0 * over1, db(lpeak * lpeak), db(bent / energy(&l, &r).max(1e-30)));
    }
}

fn section_colour() {
    let passages = [("run", passage_run()), ("chords", passage_chords())];
    println!("== colour at {:.0} Hz: long-term third-octave spectra (Hann 8192, half overlap), dry; dB re the passage's total (mean of the ears)", fs());
    println!("   run = 48 notes A1..C7 v60-109, no pedal, each 0.3 s; chords = C Am F G between C3 and C5 at v50, v80, v110, a second each");
    println!("{:<34}{}", "Hz", third_octaves().iter().map(|c| if *c < 1000.0 { format!("{c:6.0}") } else { format!("{:5.1}k", c / 1000.0) }).collect::<String>());
    let mut stock_bands: Vec<Vec<f64>> = Vec::new();
    let mut players = Player::both(true);
    for (pi, (label, (script, secs))) in passages.iter().enumerate() {
        let mut stereo_of: Vec<Vec<f64>> = Vec::new();
        for (name, piano) in players.iter_mut() {
            let (l, r) = piano.play(script, *secs);
            let (bin, spectra) = ltas(&l, &r);
            let [st, mono, side] = spectra.map(|p| octave_bands(bin, &p));
            let total: f64 = st.iter().sum();
            let st_db: Vec<f64> = st.iter().map(|x| db(x / total)).collect();
            println!("{label:<7}{name:<16} stereo   {}", row(&st_db));
            println!("{label:<7}{name:<16} mono-st  {}", row(&mono.iter().zip(&st).map(|(m, s)| db(m / s)).collect::<Vec<_>>()));
            println!("{label:<7}{name:<16} side-st  {}", row(&side.iter().zip(&st).map(|(m, s)| db(m / s)).collect::<Vec<_>>()));
            let (_, all) = ltas(&l, &r);
            let s = (bin, all[0].clone());
            let t = band(&s, 20.0, 20000.0);
            let m = (bin, all[1].clone());
            let mid_mono = band(&m, 200.0, 2000.0) / band(&s, 200.0, 2000.0);
            println!("{label:<7}{name:<16} summary: level {:+6.1} dB | above 1.5k {:+6.1} | above 4k {:+6.1} | mono-st 200-2k {:+5.1} dB | side share {:+6.1} dB",
                db(t / s.1.len() as f64), db(band(&s, 1500.0, 20000.0) / t), db(band(&s, 4000.0, 20000.0) / t), db(mid_mono), db(band(&(bin, all[2].clone()), 20.0, 20000.0) / t));
            stereo_of.push(st_db);
        }
        println!("{label:<7}{:<16} WG-CG    {}", "", row(&stereo_of[0].iter().zip(&stereo_of[1]).map(|(a, b)| a - b).collect::<Vec<_>>()));
        stock_bands.push(stereo_of.swap_remove(0));
        let _ = pi;
    }
    println!("== colour: each switch, dry: stereo band dB (re its own total) minus the stock instrument's; then its level change and share above 1.5 kHz");
    for (name, _) in SWITCHES.iter().filter(|(n, _)| !matches!(*n, "room-off" | "mono")) {
        let mut piano = variant(name, true);
        for (pi, (label, (script, secs))) in passages.iter().enumerate() {
            let (mut l, mut r) = render_in(&mut piano, script, *secs, 256);
            if *name == "mono" {
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    *a = 0.5 * (*a + *b);
                    *b = *a;
                }
            }
            let (bin, spectra) = ltas(&l, &r);
            let st = octave_bands(bin, &spectra[0]);
            let total: f64 = st.iter().sum();
            let d: Vec<f64> = st.iter().zip(&stock_bands[pi]).map(|(x, s)| db(x / total) - s).collect();
            let s = (bin, spectra[0].clone());
            let mono = (bin, spectra[1].clone());
            println!("{label:<7}{name:<18}{}{} | level {:+6.1} | above 1.5k {:+6.1} | mono-st 200-2k {:+5.1}", "", row(&d), db(total / s.1.len() as f64), db(band(&s, 1500.0, 20000.0) / band(&s, 20.0, 20000.0)), db(band(&mono, 200.0, 2000.0) / band(&s, 200.0, 2000.0)));
        }
    }

    println!("== colour: limiter load, as Score plays (room=, master 1), limiter off: peak and share of samples above the knee 0.7");
    let mut free = variant("limiter-off", false);
    for (label, (script, secs)) in [("run", passage_run()), ("chords", passage_chords()), ("soft-note", passage_soft_note()), ("forte-chord", passage_forte_chord()), ("phrase", passage_phrase())] {
        let (l, r) = render_in(&mut free, &script, secs, 256);
        let (peak, over) = limiter_load(&l, &r);
        println!("{label:<12} peak {peak:5.2} ({:+5.1} dBFS) | above 0.7: {:.4} %", db(peak * peak), 100.0 * over);
    }
    // chords per velocity layer
    let (script, _) = passage_chords();
    for (i, vel) in [50u8, 80, 110].into_iter().enumerate() {
        let part: Vec<(f64, Play)> = script.iter().filter(|(t, _)| (*t as usize) / 4 == i).map(|&(t, e)| (t - 4.0 * i as f64, e)).collect();
        let (l, r) = render_in(&mut free, &part, 4.5, 256);
        let (peak, over) = limiter_load(&l, &r);
        println!("chords v{vel:<4} peak {peak:5.2} ({:+5.1} dBFS) | above 0.7: {:.4} %", db(peak * peak), 100.0 * over);
    }

    println!("== colour: phantoms, dry single notes, 0-1 s from onset: share above 1.5 kHz (dB re the note), then the change of the level above 1.5 kHz without phantoms and with resonators only (dB)");
    let mut pianos = [stock(true), variant("phantoms-off", true), variant("phantoms-resonant", true)];
    for key in [21u8, 24, 36, 48, 60, 72, 84] {
        let mut line = format!("{:>3}", name(key));
        for vel in [28u8, 68, 87, 112] {
            let mut top = [0.0; 3];
            let mut share = 0.0;
            for (i, p) in pianos.iter_mut().enumerate() {
                let (l, r) = render_in(p, &[(0.0, Play::On(key, vel))], 1.1, 256);
                let o = onset(&l, &r);
                let s = spectrum(sect(&l[o..], 0.0, 1.0), sect(&r[o..], 0.0, 1.0));
                top[i] = band(&s, 1500.0, 20000.0);
                if i == 0 {
                    share = db(top[0] / band(&s, 20.0, 20000.0));
                }
            }
            line += &format!(" | v{vel:<3} {share:+6.1} off {:+6.1} res {:+6.1}", db(top[1] / top[0]), db(top[2] / top[0]));
        }
        println!("{line}");
    }

    println!("== colour: partial 1 against the strongest of 2-6 at 0.05-0.3 s (the fixture's p1 early), model minus recording: at the bridge, at the ears (stereo), mono, board-direct; recording's own value last");
    let rows: Vec<(u8, u8, f64)> = FIXTURE.lines().filter(|l| !l.starts_with('#')).skip(1).map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        (f[0].parse().unwrap(), f[1].parse().unwrap(), f[14].parse().unwrap())
    }).collect();
    let mut direct = variant("board-direct", true);
    let mut p = stock(true);
    let p1 = |s: &(f64, Vec<f64>), key: u8| {
        let f0 = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
        let v: Vec<f64> = (1..=6).map(|n| band(s, (n as f64 - 0.4) * f0, (n as f64 + 0.4) * f0)).collect();
        db(v[0] / v[1..].iter().copied().fold(0.0, f64::max))
    };
    let mut sums = [[0.0f64; 8]; 2];
    let mut counts = [0.0f64; 2];
    for &(key, vel, reference) in &rows {
        let bridge = p.bridge_velocity(key, vel, (0.4 * fs()) as usize);
        let o = onset(&bridge, &bridge);
        let at_bridge = p1(&spectrum(sect(&bridge[o..], 0.05, 0.3), sect(&bridge[o..], 0.05, 0.3)), key);
        let (l, r) = render_in(&mut p, &[(0.0, Play::On(key, vel))], 0.4, 256);
        let o = onset(&l, &r);
        let (l, r) = (&l[o..], &r[o..]);
        let ears = p1(&spectrum(sect(l, 0.05, 0.3), sect(r, 0.05, 0.3)), key);
        let m: Vec<f32> = l.iter().zip(r).map(|(a, b)| 0.5 * (a + b)).collect();
        let mono = p1(&spectrum(sect(&m, 0.05, 0.3), sect(&m, 0.05, 0.3)), key);
        let (dl, dr) = render_in(&mut direct, &[(0.0, Play::On(key, vel))], 0.4, 256);
        let o = onset(&dl, &dr);
        let dir = p1(&spectrum(sect(&dl[o..], 0.05, 0.3), sect(&dr[o..], 0.05, 0.3)), key);
        let d = [at_bridge - reference, ears - reference, mono - reference, dir - reference];
        let g = if key <= 48 { 0 } else { 1 };
        counts[g] += 1.0;
        for (i, x) in d.iter().enumerate() {
            sums[g][i] += x;
            sums[g][4 + i] += x.abs();
        }
        println!("{:>3} v{vel:<3} bridge {:+6.1} ears {:+6.1} mono {:+6.1} direct {:+6.1} | recording {reference:+6.1}", name(key), d[0], d[1], d[2], d[3]);
    }
    for (g, label) in ["A0..C3", "C4..C7"].iter().enumerate() {
        let c = counts[g];
        println!("{label}: mean (signed / |abs|) bridge {:+.1}/{:.1} ears {:+.1}/{:.1} mono {:+.1}/{:.1} direct {:+.1}/{:.1}", sums[g][0] / c, sums[g][4] / c, sums[g][1] / c, sums[g][5] / c, sums[g][2] / c, sums[g][6] / c, sums[g][3] / c, sums[g][7] / c);
    }

    println!("== colour: the board's sampling. Per key, partials 1-8 below 2 kHz: board power gain (mean of the ears, before regulation) with mode shapes at 4 and 8 places against at the key's own place (direct), and the mono sum against the ears' mean");
    let eight = variant("board-8", true);
    let gain = |p: &WaveguidePiano, key: u8, f: f64| {
        let [l, r] = p.board_transfer(key, f, false);
        let stereo = 0.5 * (l.0 * l.0 + l.1 * l.1 + r.0 * r.0 + r.1 * r.1);
        let mono = 0.25 * ((l.0 + r.0).powi(2) + (l.1 + r.1).powi(2));
        (stereo, mono)
    };
    let regions = [("A0..B2", 21u8, 47u8), ("C3..B4", 48, 71), ("C5..C6", 72, 84), ("C#6..C8", 85, 108)];
    for (label, lo, hi) in regions {
        let mut stats = [(0.0f64, 0.0f64, 0usize, 0.0f64); 4];
        let mut count = 0usize;
        for key in lo..=hi {
            let k = p.scale()[(key - 21) as usize].clone();
            for n in 1..=8usize {
                let f = string_partial(k.f1, k.b, n);
                if f > 2000.0 {
                    break;
                }
                let (s4, m4) = gain(&p, key, f);
                let (s8, _) = gain(&eight, key, f);
                let (sd, md) = gain(&direct, key, f);
                count += 1;
                for (i, d) in [db(s4 / sd), db(s8 / sd), db(m4 / s4), db(md / sd)].into_iter().enumerate() {
                    stats[i].0 += d;
                    stats[i].1 += d * d;
                    stats[i].2 += (d < -6.0) as usize;
                    stats[i].3 = stats[i].3.min(d);
                }
            }
        }
        let c = count as f64;
        let f = |s: (f64, f64, usize, f64)| format!("mean {:+5.1} rms {:4.1} holes>6dB {:3} worst {:+6.1}", s.0 / c, (s.1 / c).sqrt(), s.2, s.3);
        println!("{label} ({count} partials): 4 vs direct {} | 8 vs direct {} | mono-st (4) {} | mono-st (direct) {}", f(stats[0]), f(stats[1]), f(stats[2]), f(stats[3]));
    }
    for (key, n) in [(63u8, 1usize), (60, 2), (56, 2), (60, 1), (48, 1), (67, 1)] {
        let k = p.scale()[(key - 21) as usize].clone();
        let f = string_partial(k.f1, k.b, n);
        let (s4, m4) = gain(&p, key, f);
        let (s8, _) = gain(&eight, key, f);
        let (sd, md) = gain(&direct, key, f);
        println!("{:>3} partial {n} ({f:6.1} Hz): 4 places {:+6.1} dB, 8 places {:+6.1}, direct {:+6.1}; mono-st (4) {:+6.1}, (direct) {:+6.1}", name(key), db(s4), db(s8), db(sd), db(m4 / s4), db(md / sd));
    }

    println!("== colour: block size. The pedalled 48-note passage and the run, rendered in host buffers of 256 against 251, 37 and 17 samples: level of the difference re the signal, dB");
    let mut cpu_script = vec![(0.0, Play::Pedal(1.0))];
    for i in 0..48u32 {
        cpu_script.push((0.125 * i as f64, Play::On(24 + ((i * 29) % 72) as u8, 60 + ((i * 13) % 50) as u8)));
    }
    for (label, script, secs) in [("pedalled", cpu_script.clone(), 8.0), ("run", passage_run().0, 7.5)] {
        let (l0, r0) = render_in(&mut p, &script, secs, 256);
        let e0 = energy(&l0, &r0);
        let mut line = format!("{label:<9}");
        for block in [251usize, 37, 17] {
            let (l, r) = render_in(&mut p, &script, secs, block);
            let dl: Vec<f32> = l.iter().zip(&l0).map(|(a, b)| a - b).collect();
            let dr: Vec<f32> = r.iter().zip(&r0).map(|(a, b)| a - b).collect();
            line += &format!(" | {block:3}: {:+6.1} dB", db(energy(&dl, &dr) / e0));
        }
        println!("{line}");
    }

    println!("== colour: duplex segments at this rate (frequency Hz; * = held at the 0.2 fs ceiling of {:.0} Hz)", 0.2 * fs());
    for (region, freqs) in p.duplex_frequencies().iter().enumerate().filter(|(_, f)| !f.is_empty()) {
        let at_ceiling = freqs.iter().filter(|&&f| f > 0.2 * fs() - 1.0).count();
        println!("region {region}: {} | {at_ceiling} of {} at the ceiling", freqs.iter().map(|&f| format!("{f:.0}{}", if f > 0.2 * fs() - 1.0 { "*" } else { "" })).collect::<Vec<_>>().join(" "), freqs.len());
    }

    println!("== colour: CPU of the pedalled passage (room on), stock against 8 places and direct, alternately, best of 3 each");
    let mut timed = [("stock", variant("stock", false)), ("board-8", variant("board-8", false)), ("board-direct", variant("board-direct", false))];
    let mut best = [f64::MAX; 3];
    for _ in 0..3 {
        for (i, (_, piano)) in timed.iter_mut().enumerate() {
            let t = std::time::Instant::now();
            std::hint::black_box(render_in(piano, &cpu_script, 10.0, 256));
            best[i] = best[i].min(t.elapsed().as_secs_f64() / 10.0);
        }
    }
    println!("{}", timed.iter().zip(best).map(|((n, _), b)| format!("{n} {b:.3} s/s")).collect::<Vec<_>>().join(" | "));
}

/// As Score plays: device callbacks of `callback` frames, each cut at every
/// event, each piece one `process` call with its events at offset 0. With
/// `quantum`, event times are moved down to a multiple of it. Returns the
/// output and the seconds each callback took.
fn render_score(p: &mut WaveguidePiano, script: &[(f64, Play)], secs: f64, callback: usize, quantum: Option<usize>) -> (Vec<f32>, Vec<f32>, Vec<f64>) {
    let total = (secs * fs()) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut events: Vec<(usize, PianoEvent)> = script.iter().map(|&(t, e)| {
        let at = (t * fs()).round() as usize;
        (quantum.map_or(at, |q| at / q * q), e.waveguide())
    }).collect();
    events.sort_by_key(|e| e.0);
    p.reset();
    let mut times = Vec::new();
    let (mut pos, mut next) = (0, 0);
    while pos < total {
        let end = (pos + callback).min(total);
        let t = std::time::Instant::now();
        let mut at = pos;
        while at < end {
            let first = next;
            while next < events.len() && events[next].0 <= at {
                next += 1;
            }
            let piece_end = if next < events.len() { events[next].0.min(end) } else { end };
            let due: Vec<TimedEvent> = events[first..next].iter().map(|&(_, event)| TimedEvent { offset: 0, event }).collect();
            p.process(&due, &mut l[at..piece_end], &mut r[at..piece_end]);
            at = piece_end;
        }
        times.push(t.elapsed().as_secs_f64());
        pos = end;
    }
    (l, r, times)
}

/// Clicks: samples where the second difference (a steep high-pass) stands
/// more than `ratio` dB above its RMS over the 5 ms either side (the 0.25 ms
/// around it left out) and above -100 dBFS. Returns (sample, dB above the
/// local RMS, level dBFS of the jump), one per 2 ms.
fn find_clicks(x: &[f32], ratio: f64) -> Vec<(usize, f64, f64)> {
    let n = x.len();
    let mut y = vec![0.0f64; n];
    for i in 2..n {
        y[i] = x[i] as f64 - 2.0 * x[i - 1] as f64 + x[i - 2] as f64;
    }
    let mut acc = vec![0.0f64; n + 1];
    for i in 0..n {
        acc[i + 1] = acc[i] + y[i] * y[i];
    }
    let wide = (0.005 * fs()) as usize;
    let near = (0.00025 * fs()) as usize;
    let mut out: Vec<(usize, f64, f64)> = Vec::new();
    for i in wide..n.saturating_sub(wide) {
        let e = y[i] * y[i];
        if e < 1e-10 {
            continue;
        }
        let around = (acc[i + wide] - acc[i - wide]) - (acc[i + near + 1] - acc[i - near]);
        let local = around / (2 * (wide - near) - 1) as f64;
        let over = db(e / local.max(1e-30));
        if over > ratio {
            match out.last_mut() {
                Some(last) if i - last.0 < (0.002 * fs()) as usize => {
                    if over > last.1 {
                        *last = (i, over, db(e));
                    }
                }
                _ => out.push((i, over, db(e))),
            }
        }
    }
    out
}

fn section_clicks() {
    let callback = arg("callback").unwrap_or(512.0) as usize;
    println!("== clicks at {:.0} Hz, as Score plays (callbacks of {callback}, cut at every event, events at offset 0; room=, master 1): jumps of the second difference 20 dB over the 5 ms around them", fs());
    println!("   class: attack = within 5 ms after a note-on; release = within 40 ms after a note-off or the pedal coming up; other = neither");
    let passages = [("phrase", passage_phrase()), ("chords", passage_chords()), ("pedalled-48", passage_pedalled48())];
    let variants: [(&str, &str); 8] = [
        ("stock", "as built"),
        ("aligned", "event times moved down to a multiple of 32 samples: every block full and in step"),
        ("coupling-0", "no sympathetic feedback between keys"),
        ("phantoms-off", "no phantoms"),
        ("limiter-off", "no limiter"),
        ("duplex-off", "no duplex"),
        ("damper-slow", "release with every key's damper ramp 10x longer (set_damper_slew)"),
        ("knock-0", "no case knock"),
    ];
    for (label, (script, secs)) in &passages {
        let ons: Vec<usize> = script.iter().filter_map(|&(t, e)| matches!(e, Play::On(..)).then_some((t * fs()).round() as usize)).collect();
        let offs: Vec<(usize, String)> = script.iter().filter_map(|&(t, e)| match e {
            Play::Off(k) => Some(((t * fs()).round() as usize, format!("off {}", name(k)))),
            Play::Pedal(v) if v < 0.5 => Some(((t * fs()).round() as usize, "pedal up".to_string())),
            _ => None,
        }).collect();
        for (vname, _) in variants {
            let mut p = variant(vname, false);
            let (l, r, times) = render_score(&mut p, script, *secs, callback, (vname == "aligned").then_some(32));
            let mut found = find_clicks(&l, 20.0);
            for c in find_clicks(&r, 20.0) {
                if !found.iter().any(|f| f.0.abs_diff(c.0) < (0.002 * fs()) as usize) {
                    found.push(c);
                }
            }
            found.sort_by_key(|c| c.0);
            let classify = |i: usize| -> (String, String) {
                if ons.iter().any(|&t| i >= t && i < t + (0.005 * fs()) as usize) {
                    return ("attack".into(), String::new());
                }
                if let Some((t, what)) = offs.iter().filter(|(t, _)| i >= *t && i < t + (0.04 * fs()) as usize).last() {
                    return ("release".into(), format!("{what} +{:.1} ms", (i - t) as f64 * 1e3 / fs()));
                }
                let nearest = ons.iter().map(|&t| i as i64 - t as i64).min_by_key(|d| d.abs()).unwrap_or(0);
                ("other".into(), format!("nearest note-on {:+.1} ms", nearest as f64 * 1e3 / fs()))
            };
            let mut counts = std::collections::BTreeMap::new();
            let mut shown = Vec::new();
            for &(i, over, level) in &found {
                let (class, why) = classify(i);
                *counts.entry(class.clone()).or_insert(0usize) += 1;
                if class != "attack" && shown.len() < 10 {
                    shown.push(format!("{:.4} s {over:+5.1} dB {level:+6.1} dBFS {class} {why}", i as f64 / fs()));
                }
            }
            let budget = callback as f64 / fs();
            let worst = times.iter().copied().fold(0.0, f64::max);
            let mut sorted = times.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p99 = sorted[(sorted.len() * 99 / 100).min(sorted.len() - 1)];
            println!("{label:<12}{vname:<13} clicks {:?} | callback time worst {:.0} % of its {:.1} ms, p99 {:.0} %", counts, 100.0 * worst / budget, budget * 1e3, 100.0 * p99 / budget);
            if vname == "stock" || vname == "aligned" || vname == "coupling-0" {
                for line in &shown {
                    println!("      {line}");
                }
            }
        }
    }
}

/// A0, C1, E1, F#1, A1 and C2 alone at `vel`, 4 s apart, each held 3.6 s.
fn passage_bassnotes(vel: u8) -> (Vec<(f64, Play)>, f64) {
    let mut script = Vec::new();
    for (i, k) in BASS_KEYS.into_iter().enumerate() {
        script.push((4.0 * i as f64, Play::On(k, vel)));
        script.push((4.0 * i as f64 + 3.6, Play::Off(k)));
    }
    (script, 4.0 * BASS_KEYS.len() as f64)
}

const BASS_KEYS: [u8; 6] = [21, 24, 28, 30, 33, 36];
const TOUCH_KEYS: [u8; 4] = [33, 48, 60, 84];
const TOUCH_VELS: [u8; 4] = [28, 68, 87, 112];

/// A1, C3, C4 and C6 at velocities 28, 68, 87 and 112, 2.5 s apart, each held 2 s.
fn passage_touch() -> (Vec<(f64, Play)>, f64) {
    let mut script = Vec::new();
    let mut t = 0.0;
    for k in TOUCH_KEYS {
        for v in TOUCH_VELS {
            script.push((t, Play::On(k, v)));
            script.push((t + 2.0, Play::Off(k)));
            t += 2.5;
        }
    }
    (script, t)
}

/// Every passage the reference is rendered with, by label.
fn reference_passages() -> Vec<(&'static str, (Vec<(f64, Play)>, f64))> {
    vec![
        ("run", passage_run()),
        ("chords", passage_chords()),
        ("soft-note", passage_soft_note()),
        ("forte-chord", passage_forte_chord()),
        ("phrase", passage_phrase()),
        ("bass", passage_bass()),
        ("dense", passage_dense()),
        ("bassnotes-v87", passage_bassnotes(87)),
        ("bassnotes-v68", passage_bassnotes(68)),
        ("touch", passage_touch()),
    ]
}

/// The passages as scripts for the Audio Unit harness (au_compare --script).
fn section_scripts() {
    let dir = std::path::PathBuf::from(std::env::var_os("PROBE_OUT").expect("PROBE_OUT"));
    std::fs::create_dir_all(&dir).expect("output dir");
    for (label, (script, secs)) in reference_passages() {
        let mut text = String::new();
        for (t, e) in script {
            text += &match e {
                Play::On(k, v) => format!("{t:.4} on {k} {v}\n"),
                Play::Off(k) => format!("{t:.4} off {k}\n"),
                Play::Pedal(v) => format!("{t:.4} pedal {v}\n"),
            };
        }
        text += &format!("end {secs:.4}\n");
        std::fs::write(dir.join(format!("{label}.txt")), text).expect("write script");
        println!("{label}.txt");
    }
}

/// A stereo PCM (16 or 24 bit) or float WAV: (left, right, rate).
fn read_wav(path: &std::path::Path) -> Option<(Vec<f32>, Vec<f32>, u32)> {
    let bytes = std::fs::read(path).ok()?;
    let (mut pos, mut format, mut channels, mut rate, mut bits) = (12usize, 0u16, 0u16, 0u32, 0u16);
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().ok()?) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            format = u16::from_le_bytes([body[0], body[1]]);
            channels = u16::from_le_bytes([body[2], body[3]]);
            rate = u32::from_le_bytes(body[4..8].try_into().ok()?);
            bits = u16::from_le_bytes([body[14], body[15]]);
        } else if id == b"data" {
            let width = bits as usize / 8;
            let frame = width * channels as usize;
            let (mut l, mut r) = (Vec::new(), Vec::new());
            for f in body.chunks_exact(frame) {
                let sample = |c: usize| -> f32 {
                    let b = &f[c * width..(c + 1) * width];
                    match (format, bits) {
                        (3, 32) => f32::from_le_bytes(b.try_into().unwrap()),
                        (_, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                        (_, 24) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
                        _ => 0.0,
                    }
                };
                l.push(sample(0));
                r.push(sample(if channels > 1 { 1 } else { 0 }));
            }
            return Some((l, r, rate));
        }
        pos += 8 + len + (len & 1);
    }
    None
}

/// Partials of a note found in its own spectrum: walks up from the first,
/// predicting each from the stretch measured so far. Returns (n, Hz).
fn track_partials(l: &[f32], r: &[f32], key: u8, top: f64) -> Vec<(usize, f64)> {
    let s = spectrum(sect(l, 0.05, 1.05), sect(r, 0.05, 1.05));
    let nominal = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
    let peak = |lo: f64, hi: f64| -> f64 {
        let (a, b) = (((lo / s.0) as usize).max(1), ((hi / s.0) as usize).min(s.1.len() - 1));
        let k = (a..=b).max_by(|&i, &j| s.1[i].total_cmp(&s.1[j])).unwrap_or(a);
        k as f64 * s.0
    };
    let f0 = peak(0.93 * nominal, 1.07 * nominal);
    let mut out = vec![(1usize, f0)];
    let mut b = 0.0f64;
    let mut estimates: Vec<f64> = Vec::new();
    for n in 2..=60usize {
        let want = n as f64 * f0 * (1.0 + b * (n * n) as f64).sqrt();
        if want > top {
            break;
        }
        let f = peak(want - 0.3 * f0, want + 0.3 * f0);
        out.push((n, f));
        if n >= 4 {
            estimates.push((((f / (n as f64 * f0)).powi(2) - 1.0) / (n * n) as f64).max(0.0));
            let mut sorted = estimates.clone();
            sorted.sort_by(f64::total_cmp);
            b = sorted[sorted.len() / 2];
        }
    }
    out
}

fn section_reference() {
    let dir = std::path::PathBuf::from(std::env::var_os("REF_DIR").expect("REF_DIR: the au_compare --out of the scripts"));
    println!("== reference at {:.0} Hz: R = the reference AU (its own default state), C = the Concert Grand (dry, through the harness), W = the Waveguide Grand as Score plays it (dry, callbacks of 512)", fs());
    let mut wg = stock(true);
    let load = |label: &str, side: &str| read_wav(&dir.join(format!("{label}.{side}.wav"))).map(|(l, r, _)| (l, r));
    println!("{:<30}{}", "Hz", third_octaves().iter().map(|c| if *c < 1000.0 { format!("{c:6.0}") } else { format!("{:5.1}k", c / 1000.0) }).collect::<String>());
    for (label, (script, secs)) in reference_passages().into_iter().filter(|(l, _)| !l.starts_with("bassnotes") && *l != "touch") {
        let (Some(rf), Some(cg)) = (load(label, "B"), load(label, "A")) else {
            println!("{label}: no reference render");
            continue;
        };
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        let mut rows: Vec<(&str, Vec<f64>, Vec<f64>, (f64, f64, f64))> = Vec::new();
        for (who, (l, r)) in [("R", (&rf.0, &rf.1)), ("W", (&wl, &wr)), ("C", (&cg.0, &cg.1))] {
            let (bin, spectra) = ltas(l, r);
            let [st, mono, _] = spectra.clone().map(|p| octave_bands(bin, &p));
            let total: f64 = st.iter().sum();
            let s = (bin, spectra[0].clone());
            let m = (bin, spectra[1].clone());
            let t = band(&s, 20.0, 20000.0);
            rows.push((who, st.iter().map(|x| db(x / total)).collect(), mono.iter().zip(&st).map(|(m, s)| db(m / s)).collect(),
                (db(band(&s, 1500.0, 20000.0) / t), db(band(&s, 4000.0, 20000.0) / t), db(band(&m, 200.0, 2000.0) / band(&s, 200.0, 2000.0)))));
        }
        println!("{label:<12}R stereo        {}", row(&rows[0].1));
        println!("{label:<12}W minus R       {}", row(&rows[1].1.iter().zip(&rows[0].1).map(|(a, b)| a - b).collect::<Vec<_>>()));
        println!("{label:<12}C minus R       {}", row(&rows[2].1.iter().zip(&rows[0].1).map(|(a, b)| a - b).collect::<Vec<_>>()));
        println!("{label:<12}R mono-st       {}", row(&rows[0].2));
        println!("{label:<12}W mono-st       {}", row(&rows[1].2));
        for (who, _, _, (h15, h4, ms)) in &rows {
            print!("  {who}: above 1.5k {h15:+6.1} above 4k {h4:+6.1} mono-st 200-2k {ms:+5.1} |");
        }
        println!();
        if matches!(label, "dense" | "phrase" | "chords") {
            let ons: Vec<usize> = script.iter().filter_map(|&(t, e)| matches!(e, Play::On(..)).then_some((t * fs()).round() as usize)).collect();
            let mut line = format!("{label:<12}clicks (attack / other):");
            for (who, (l, r)) in [("R", (&rf.0, &rf.1)), ("W", (&wl, &wr)), ("C", (&cg.0, &cg.1))] {
                let mut found = find_clicks(l, 20.0);
                found.extend(find_clicks(r, 20.0));
                let attack = found.iter().filter(|c| ons.iter().any(|&t| c.0 >= t && c.0 < t + (0.005 * fs()) as usize)).count();
                line += &format!(" {who} {attack} / {}", found.len() - attack);
            }
            println!("{line}");
        }
    }

    for vel in [87u8, 68] {
        let label = format!("bassnotes-v{vel}");
        let Some(rf) = load(&label, "B") else { continue };
        let cg = load(&label, "A");
        println!("== reference: bass v{vel}, decay dB/s over 0.1-1.5 s of partials 1..40 below 8 kHz (R found in its own spectrum; then its stretch B), W at its own partials, C found in its spectrum");
        for (i, key) in BASS_KEYS.into_iter().enumerate() {
            let t0 = 4.0 * i as f64;
            let part = |x: &Vec<f32>| -> Vec<f32> { sect(x, t0, t0 + 3.5).to_vec() };
            let show = |who: &str, l: Vec<f32>, r: Vec<f32>, partials: Vec<(usize, f64)>| {
                let o = onset(&l, &r);
                let (l, r) = (&l[o..], &r[o..]);
                let f0 = partials[0].1;
                let decays: Vec<f64> = partials.iter().map(|&(_, f)| band_decay(l, r, f, 0.4 * f0).0).collect();
                let b = partials.last().map_or(0.0, |&(n, f)| (((f / (n as f64 * f0)).powi(2) - 1.0) / (n * n) as f64).max(0.0));
                let mean = |a: usize, z: usize| decays.get(a - 1..z.min(decays.len())).map_or(f64::NAN, |d| d.iter().sum::<f64>() / d.len().max(1) as f64);
                println!("{:>3} {who} {} | n10-20 {:.1} n21-40 {:.1} | B {b:.2e}", name(key), decays.iter().map(|d| format!("{d:5.1}")).collect::<String>(), mean(10, 20), mean(21, 40));
            };
            let (rl, rr) = (part(&rf.0), part(&rf.1));
            let tracked = track_partials(&rl, &rr, key, 8000.0);
            show("R", rl, rr, tracked);
            let k = wg.scale()[(key - 21) as usize].clone();
            let (wl, wr) = render_in(&mut wg, &[(0.0, Play::On(key, vel))], 3.5, 256);
            show("W", wl, wr, (1..=60).map(|n| (n, string_partial(k.f1, k.b, n))).take_while(|p| p.1 < 8000.0).collect());
            if let Some(cg) = &cg {
                let (cl, cr) = (part(&cg.0), part(&cg.1));
                let tracked = track_partials(&cl, &cr, key, 8000.0);
                show("C", cl, cr, tracked);
            }
        }
    }

    if let Some(rf) = load("touch", "B") {
        println!("== reference: touch. Per key and velocity: level dB 0-1.5 s re v68, share above 2 kHz at 0.02-0.5 s, spectral centroid of the first 30 ms, 10-90 % rise of the 1 ms envelope (ms)");
        let mut t = 0.0;
        let (script, secs) = passage_touch();
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        for key in TOUCH_KEYS {
            let mut lines = [format!("{:>3} R", name(key)), format!("{:>3} W", name(key))];
            let mut levels = [[0.0; 4]; 2];
            let mut cells = [Vec::new(), Vec::new()];
            for (vi, _) in TOUCH_VELS.iter().enumerate() {
                for (w, (l, r)) in [(&rf.0, &rf.1), (&wl, &wr)].into_iter().enumerate() {
                    let (l, r) = (sect(l, t, t + 2.4), sect(r, t, t + 2.4));
                    let o = onset(l, r);
                    let (l, r) = (&l[o..], &r[o..]);
                    levels[w][vi] = db(energy(sect(l, 0.0, 1.5), sect(r, 0.0, 1.5)));
                    let s = spectrum(sect(l, 0.02, 0.5), sect(r, 0.02, 0.5));
                    let share = db(band(&s, 2000.0, 20000.0) / band(&s, 20.0, 20000.0));
                    let (_, _, cen) = band_line(l, r, 0.0, 0.03);
                    let env: Vec<f64> = (0..60).map(|i| energy(sect(l, i as f64 * 0.001, (i + 1) as f64 * 0.001), sect(r, i as f64 * 0.001, (i + 1) as f64 * 0.001))).collect();
                    let top = env.iter().copied().fold(0.0, f64::max);
                    let a = env.iter().position(|&e| e > 0.01 * top).unwrap_or(0);
                    let b = env.iter().position(|&e| e > 0.81 * top).unwrap_or(0);
                    cells[w].push((share, cen, b.saturating_sub(a)));
                }
                t += 2.5;
            }
            for w in 0..2 {
                for (vi, v) in TOUCH_VELS.iter().enumerate() {
                    let (share, cen, rise) = cells[w][vi];
                    lines[w] += &format!(" | v{v:<3} {:+5.1} dB {share:+6.1} {cen:5.0} Hz {rise:2} ms", levels[w][vi] - levels[w][1]);
                }
                println!("{}", lines[w]);
            }
        }
    }
}

/// Where the bass's spikes are born: the same detector on a key's bridge
/// velocity alone (strings and hammer, before the board), on the output, and
/// on the output without the board's direct path, per switch.
fn section_spikes() {
    let vel = arg("vel").unwrap_or(87.0) as u8;
    println!("== spikes v{vel}, dry, one key from rest, 0.25 s: ms after the strike of each jump of the second difference 20 dB over the 5 ms around it; hammer contact ms and peak N");
    for name_ in ["stock", "phantoms-off", "sideways-0", "loss-high", "yield-1", "direct-0", "knock-0", "duplex-off"] {
        let mut p = variant(name_, true);
        for key in [21u8, 24, 28, 33, 40] {
            let len = (0.25 * fs()) as usize;
            let v = p.bridge_velocity(key, vel, len);
            let (contact, force) = p.last_blow(key);
            let (l, r) = render_in(&mut p, &[(0.0, Play::On(key, vel))], 0.25, 256);
            let ms = |c: &[(usize, f64, f64)]| c.iter().map(|c| format!("{:.1}", c.0 as f64 * 1e3 / fs())).collect::<Vec<_>>().join(" ");
            let mut out = find_clicks(&l, 20.0);
            out.extend(find_clicks(&r, 20.0));
            out.sort_by_key(|c| c.0);
            out.dedup_by(|a, b| a.0.abs_diff(b.0) < (0.002 * fs()) as usize);
            println!("{name_:<13}{:>3} contact {:.2} ms {force:5.1} N | bridge [{}] | output [{}]", name(key), contact * 1e3, ms(&find_clicks(&v, 20.0)), ms(&out));
        }
    }
}

/// Every key at six strengths: how the felt meets the strings (each contact's
/// start, end and peak) and whether the bridge velocity carries spikes (the
/// click detector on 0.25 s of one key from rest, dry, no board).
fn section_contacts() {
    let mut p = variant("stock", true);
    let vels = [40u8, 60, 75, 87, 100, 112];
    let len = (0.25 * fs()) as usize;
    let ms = |i: usize| i as f64 * 1e3 / fs();
    println!("== contacts: per key and velocity, bridge spikes (count, first ms) and the hammer's contacts (start-end ms, peak N); keys with no spike at any strength in one line at the end");
    let mut clean = Vec::new();
    let mut table = Vec::new();
    for key in 21u8..=108 {
        let mut row = String::new();
        let mut any = false;
        let mut counts = Vec::new();
        for &vel in &vels {
            let (v, force) = p.blow_trace(key, vel, len);
            let spikes = find_clicks(&v, 20.0);
            let mut contacts = Vec::new();
            let mut start = None;
            let mut peak = 0.0f32;
            for (i, &f) in force.iter().enumerate() {
                if f > 0.0 {
                    if start.is_none() {
                        start = Some(i);
                        peak = 0.0;
                    }
                    peak = peak.max(f);
                } else if let Some(s) = start.take() {
                    contacts.push((s, i, peak));
                }
            }
            if let Some(s) = start {
                contacts.push((s, force.len(), peak));
            }
            counts.push(spikes.len());
            if !spikes.is_empty() {
                any = true;
            }
            let c = contacts.iter().map(|c| format!("{:.2}-{:.2}/{:.0}", ms(c.0), ms(c.1), c.2)).collect::<Vec<_>>().join(" ");
            row += &format!(" | v{vel} {:2}{} {c}", spikes.len(), spikes.first().map_or(String::new(), |s| format!("@{:.1}", ms(s.0))));
        }
        table.push((key, counts.clone()));
        if any {
            println!("{:>3}{row}", name(key));
        } else {
            clean.push(name(key));
        }
    }
    println!("no spikes: {}", clean.join(" "));
    print!("spike counts per key (v40 v60 v75 v87 v100 v112):");
    for (key, c) in &table {
        if c.iter().any(|&n| n > 0) {
            print!(" {}={:?}", name(*key), c);
        }
    }
    println!();
}

/// 48 notes over six seconds, all over the keyboard, under the pedal: the
/// CPU case.
fn passage_pedalled48() -> (Vec<(f64, Play)>, f64) {
    let mut script = vec![(0.0, Play::Pedal(1.0))];
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % 72) as u8;
        script.push((0.125 * i as f64, Play::On(key, 60 + ((i * 13) % 50) as u8)));
        script.push((0.125 * i as f64 + 0.4, Play::Off(key)));
    }
    script.push((7.0, Play::Pedal(0.0)));
    (script, 8.5)
}

/// Callback times as Score drives the instrument, three passes in a row in
/// one process: a spike only in the first pass is warm-up (JIT tier-up, cold
/// caches), one in every pass is the instrument's own. A fourth pass on a
/// newly built instrument tells the process's warm-up (code) from the
/// instrument's own (its memory touched for the first time).
/// `TIMING=dense|phrase|pedalled-48` runs that passage alone, first in
/// the process. `WARM_UP=1` first warms each built instrument up as Score
/// does (a mezzo-forte note with the pedal down through one callback, then
/// silence).
fn section_timing() {
    let callback = arg("callback").unwrap_or(512.0) as usize;
    let budget = callback as f64 / fs();
    let only = std::env::var("TIMING").ok();
    let warm_up = std::env::var("WARM_UP").is_ok_and(|w| w == "1");
    let build = || {
        let mut p = variant("stock", true);
        if warm_up {
            let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
            let events = [TimedEvent { offset: 0, event: PianoEvent::Sustain { value: 1.0 } }, TimedEvent { offset: 0, event: PianoEvent::NoteOn { key: 60, velocity: 64 } }];
            let t = std::time::Instant::now();
            p.process(&events, &mut l, &mut r);
            println!("   warm-up callback: {:.0} % of the budget", 100.0 * t.elapsed().as_secs_f64() / (512.0 / fs()));
            p.reset();
        }
        p
    };
    println!("== timing at {:.0} Hz, callbacks of {callback} ({:.1} ms): per pass worst, p99.9 and p99 in % of the budget, and where the three worst fell (s)", fs(), budget * 1e3);
    for (label, (script, secs)) in [("dense", passage_dense()), ("phrase", passage_phrase()), ("pedalled-48", passage_pedalled48())] {
        if only.as_deref().is_some_and(|o| o != label) {
            continue;
        }
        let mut p = build();
        for pass in 1..=4 {
            if pass == 4 {
                p = build();
            }
            let (_, _, times) = render_score(&mut p, &script, secs, callback, None);
            let mut sorted = times.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let at = |q: f64| 100.0 * sorted[((sorted.len() as f64 * q) as usize).min(sorted.len() - 1)] / budget;
            let mut order: Vec<usize> = (0..times.len()).collect();
            order.sort_by(|&a, &b| times[b].partial_cmp(&times[a]).unwrap());
            let worst = order.iter().take(3).map(|&i| format!("{:.2} s {:.0} %", (i * callback) as f64 / fs(), 100.0 * times[i] / budget)).collect::<Vec<_>>().join(", ");
            println!("{label:<11} pass {}: worst {:4.0} % p99.9 {:4.0} % p99 {:4.0} % mean {:4.1} % | {worst}", if pass == 4 { "4 (new instrument)".to_string() } else { pass.to_string() }, at(1.0), at(0.999), at(0.99), 100.0 * times.iter().sum::<f64>() / times.len() as f64 / budget);
        }
    }
}

/// A script in the au_compare harness's format: `<s> on <key> <vel>`,
/// `<s> off <key>`, `<s> pedal <0..1>`, `end <s>`.
fn read_script(path: &std::path::Path) -> Option<(Vec<(f64, Play)>, f64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let (mut script, mut end) = (Vec::new(), 0.0);
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match (w.first().copied(), w.get(1).copied()) {
            (Some("end"), _) => end = w[1].parse().ok()?,
            (_, Some("on")) => script.push((w[0].parse().ok()?, Play::On(w[2].parse().ok()?, w[3].parse().ok()?))),
            (_, Some("off")) => script.push((w[0].parse().ok()?, Play::Off(w[2].parse().ok()?))),
            (_, Some("pedal")) => script.push((w[0].parse().ok()?, Play::Pedal(w[2].parse().ok()?))),
            _ => {}
        }
    }
    Some((script, end))
}

/// Levels in dB of a band around `f` in 0.2 s windows every 0.1 s over
/// `a..b` seconds: (decay dB/s, rms of the levels about that straight line).
fn band_fit(l: &[f32], r: &[f32], f: f64, half: f64, a: f64, b: f64) -> (f64, f64) {
    let times: Vec<f64> = (0..).map(|i| a + 0.1 * i as f64).take_while(|t| t + 0.2 <= b).collect();
    let level: Vec<f64> = times.iter().map(|&t| db(band(&spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2)), f - half, f + half))).collect();
    let k = slope(&times, &level);
    let (mt, ml) = (times.iter().sum::<f64>() / times.len() as f64, level.iter().sum::<f64>() / level.len() as f64);
    let rms = (times.iter().zip(&level).map(|(t, y)| (y - (ml + k * (t - mt))).powi(2)).sum::<f64>() / times.len() as f64).sqrt();
    (-k, rms)
}

const TRAUM_KEYS: [u8; 11] = [29, 34, 36, 38, 39, 40, 41, 42, 43, 45, 48];

/// Bar 2 of Träumerei (whose held F2 sounds like a sitar) and bar 22 (whose
/// A#1 sounds right), and held bass notes F1..C3, against the reference AU.
/// TRAUM_DIR holds scripts/<label>.txt and the harness's out/<label>.{A,B}.wav;
/// ours are written beside them as out/<label>.W.wav.
fn section_traum() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let out = dir.join("out");
    let mut wg = stock(true);
    println!("== traum at {:.0} Hz: design key by key: f1, B, Z per string, strings, length m, bridge R, b1, b3, allpass sections, worst partial error (cents) to n=16/32/64, hammer g / p / contact target ms, longitudinal Hz, bridge spikes v60/v87", fs());
    for key in TRAUM_KEYS {
        println!("{}", design_row(&mut wg, key));
    }
    println!("{:<30}{}", "Hz", third_octaves().iter().map(|c| if *c < 1000.0 { format!("{c:6.0}") } else { format!("{:5.1}k", c / 1000.0) }).collect::<String>());
    for label in ["traum-open", "traum-bar22"] {
        let Some((script, secs)) = read_script(&dir.join("scripts").join(format!("{label}.txt"))) else {
            println!("{label}: no script");
            continue;
        };
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        write_wav(&out.join(format!("{label}.W.wav")), &wl, &wr);
        let Some((rl, rr, _)) = read_wav(&out.join(format!("{label}.B.wav"))) else { continue };
        let mut rows = Vec::new();
        for (l, r) in [(&rl, &rr), (&wl, &wr)] {
            let (bin, spectra) = ltas(l, r);
            let st = octave_bands(bin, &spectra[0]);
            let total: f64 = st.iter().sum();
            let s = (bin, spectra[0].clone());
            let t = band(&s, 20.0, 20000.0);
            rows.push((st.iter().map(|x| db(x / total)).collect::<Vec<_>>(), db(band(&s, 1500.0, 20000.0) / t), db(band(&s, 4000.0, 20000.0) / t)));
        }
        println!("{label:<12}R stereo        {}", row(&rows[0].0));
        println!("{label:<12}W minus R       {}", row(&rows[1].0.iter().zip(&rows[0].0).map(|(a, b)| a - b).collect::<Vec<_>>()));
        println!("{label:<12}above 1.5k R {:+.1} W {:+.1} | above 4k R {:+.1} W {:+.1} | clicks R {} W {}", rows[0].1, rows[1].1, rows[0].2, rows[1].2,
            find_clicks(&rl, 20.0).len() + find_clicks(&rr, 20.0).len(), find_clicks(&wl, 20.0).len() + find_clicks(&wr, 20.0).len());
    }
    for vel in [87u8, 60] {
        let label = format!("held-v{vel}");
        let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&out.join(format!("{label}.B.wav")))) else {
            println!("{label}: missing");
            continue;
        };
        let cg = read_wav(&out.join(format!("{label}.A.wav")));
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        write_wav(&out.join(format!("{label}.W.wav")), &wl, &wr);
        println!("== {label}: each key held 4 s, as Score plays it (W) against the reference AU (R) and the Concert Grand (C). Decay dB/s of partials, early (0.1-1.5 s) | late (1.5-3.9 s); share above 4 kHz early (0.05-1 s) / late (2-3.9 s), dB re the note; B (R and C from their measured stretch); shimmer = rms dB of partials 10-30 about their straight decay, 0.2-3.9 s");
        for (i, &key) in TRAUM_KEYS.iter().enumerate() {
            let t0 = 5.0 * i as f64;
            let k = wg.scale()[(key - 21) as usize].clone();
            let mut sides: Vec<(&str, Vec<f32>, Vec<f32>, Vec<(usize, f64)>)> = Vec::new();
            for (who, l, r) in [("R", &rl, &rr), ("W", &wl, &wr)].into_iter().chain(cg.as_ref().map(|c| ("C", &c.0, &c.1))) {
                let (l, r) = (sect(l, t0, t0 + 4.6).to_vec(), sect(r, t0, t0 + 4.6).to_vec());
                let o = onset(&l, &r);
                let (l, r) = (l[o..].to_vec(), r[o..].to_vec());
                let partials = if who == "W" {
                    (1..=60).map(|n| (n, string_partial(k.f1, k.b, n))).take_while(|p| p.1 < 8000.0).collect()
                } else {
                    track_partials(&l, &r, key, 8000.0)
                };
                sides.push((who, l, r, partials));
            }
            for (who, l, r, partials) in &sides {
                let f0 = partials[0].1;
                let early: Vec<(f64, f64)> = partials.iter().map(|&(_, f)| band_fit(l, r, f, 0.4 * f0, 0.1, 1.5)).collect();
                let late: Vec<(f64, f64)> = partials.iter().map(|&(_, f)| band_fit(l, r, f, 0.4 * f0, 1.5, 3.9)).collect();
                let whole: Vec<(f64, f64)> = partials.iter().map(|&(_, f)| band_fit(l, r, f, 0.4 * f0, 0.2, 3.9)).collect();
                let mean = |v: &[(f64, f64)], a: usize, z: usize, pick: fn(&(f64, f64)) -> f64| v.get(a - 1..z.min(v.len())).map_or(f64::NAN, |d| d.iter().map(pick).sum::<f64>() / d.len().max(1) as f64);
                let dec = |p: &(f64, f64)| p.0;
                let res = |p: &(f64, f64)| p.1;
                let tracked: Vec<f64> = partials.iter().filter(|p| p.0 >= 8 && p.0 <= 30).map(|&(n, f)| (((f / (n as f64 * f0)).powi(2) - 1.0) / (n * n) as f64).max(0.0)).collect();
                let mut sorted = tracked.clone();
                sorted.sort_by(f64::total_cmp);
                let b = if *who == "W" { k.b } else { sorted.get(sorted.len() / 2).copied().unwrap_or(0.0) };
                let share = |a: f64, z: f64| {
                    let s = spectrum(sect(l, a, z), sect(r, a, z));
                    db(band(&s, 4000.0, 20000.0) / band(&s, 20.0, 20000.0))
                };
                println!("{:>3} v{vel} {who} | early n1-8 {:5.1} n10-20 {:5.1} n21-40 {:5.1} | late n1-8 {:5.1} n10-20 {:5.1} n21-40 {:5.1} | 4k {:+6.1} / {:+6.1} | B {b:.2e} | shimmer {:4.2} dB | clicks {}",
                    name(key), mean(&early, 1, 8, dec), mean(&early, 10, 20, dec), mean(&early, 21, 40, dec), mean(&late, 1, 8, dec), mean(&late, 10, 20, dec), mean(&late, 21, 40, dec),
                    share(0.05, 1.0), share(2.0, 3.9), mean(&whole, 10, 30, res), find_clicks(l, 20.0).len() + find_clicks(r, 20.0).len());
                if matches!(key, 29 | 34 | 40 | 41) {
                    println!("      early 10-40 {}", early.iter().skip(9).take(31).map(|d| format!("{:5.1}", d.0)).collect::<String>());
                    println!("      late  10-40 {}", late.iter().skip(9).take(31).map(|d| format!("{:5.1}", d.0)).collect::<String>());
                }
            }
        }
    }
}

/// Where a held bass note's treble comes from: the share above 4 kHz (dB re
/// the note) early (0.05-1 s) and late (2-3.9 s) of the bridge velocity
/// alone, then of the dry output as built and with one thing switched off.
fn section_hfsource() {
    let vel = arg("vel").unwrap_or(87.0) as u8;
    let share = |l: &[f32], r: &[f32], a: f64, z: f64| {
        let s = spectrum(sect(l, a, z), sect(r, a, z));
        db(band(&s, 4000.0, 20000.0) / band(&s, 20.0, 20000.0))
    };
    let names: Vec<&str> = if std::env::args().any(|a| a == "felt") {
        vec!["felt-uncapped", "felt-cap-1.5", "stock"]
    } else {
        vec!["stock", "direct-0", "duplex-off", "phantoms-off", "sideways-0", "board-direct", "loss-high"]
    };
    println!("== hfsource v{vel}: above 4 kHz, early / late, dB re the note: bridge velocity, then the output of {}", names.join(", "));
    let mut pianos: Vec<(&str, WaveguidePiano)> = names.iter().map(|n| (*n, variant(n, true))).collect();
    for (n, p) in pianos.iter() {
        let felt = p.felt_at_mm();
        println!("{n:<14} felt N at 1 mm: {}", (21u8..=60).map(|k| format!("{}:{:.0}", name(k), felt[(k - 21) as usize])).collect::<Vec<_>>().join(" "));
    }
    for key in [21u8, 24, 25, 28, 31, 33, 34, 36, 37, 38, 39, 40, 41, 42, 43, 45, 48, 52] {
        let len = (4.0 * fs()) as usize;
        let (v, _) = pianos[0].1.blow_trace(key, vel, len);
        let mut line = format!("{:>3} bridge {:+6.1} / {:+6.1} |", name(key), share(&v, &v, 0.05, 1.0), share(&v, &v, 2.0, 3.9));
        for (n, p) in pianos.iter_mut() {
            let (l, r) = render_in(p, &[(0.0, Play::On(key, vel))], 4.0, 256);
            line += &format!(" {n} {:+6.1} / {:+6.1} |", share(&l, &r, 0.05, 1.0), share(&l, &r, 2.0, 3.9));
        }
        println!("{line}");
    }
}

/// Decays against the reference AU's, key by key at v60 (the bass-v60 and
/// treble-v60 scripts under TRAUM_DIR: A0-B3 every 3 s held 2 s, C4-C8
/// every 2.5 s held 1.5 s): the
/// mean decay in dB/s over 0.1-1.5 s of partials 1-4, 10-20 and 21-40 below
/// 8 kHz, R at the partials found in its own spectrum, W at its strings'.
#[derive(Clone, Copy, Debug)]
struct C(f64, f64);

impl C {
    fn add(self, o: C) -> C { C(self.0 + o.0, self.1 + o.1) }
    fn sub(self, o: C) -> C { C(self.0 - o.0, self.1 - o.1) }
    fn mul(self, o: C) -> C { C(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0) }
    fn conj(self) -> C { C(self.0, -self.1) }
    fn abs(self) -> f64 { self.0.hypot(self.1) }
    fn div(self, o: C) -> C { let d = o.0 * o.0 + o.1 * o.1; C((self.0 * o.0 + self.1 * o.1) / d, (self.1 * o.0 - self.0 * o.1) / d) }
    fn sqrt(self) -> C { let (r, a) = (self.abs().sqrt(), self.1.atan2(self.0) / 2.0); C(r * a.cos(), r * a.sin()) }
}

/// Least squares for `m` (rows) x = `b`, by the normal equations and
/// elimination with pivoting.
fn complex_lstsq(m: &[Vec<C>], b: &[C]) -> Vec<C> {
    let p = m[0].len();
    let mut a = vec![vec![C(0.0, 0.0); p + 1]; p];
    for (row, &y) in m.iter().zip(b) {
        for i in 0..p {
            for j in 0..p {
                a[i][j] = a[i][j].add(row[i].conj().mul(row[j]));
            }
            a[i][p] = a[i][p].add(row[i].conj().mul(y));
        }
    }
    for c in 0..p {
        let piv = (c..p).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs())).unwrap();
        a.swap(c, piv);
        for r in 0..p {
            if r != c && a[c][c].abs() > 0.0 {
                let f = a[r][c].div(a[c][c]);
                for k in c..=p {
                    a[r][k] = a[r][k].sub(f.mul(a[c][k]));
                }
            }
        }
    }
    (0..p).map(|i| a[i][p].div(a[i][i])).collect()
}

/// Roots of z^p - a1 z^(p-1) - ... - ap (Durand-Kerner).
fn lp_roots(a: &[C]) -> Vec<C> {
    let p = a.len();
    let poly = |z: C| a.iter().fold(C(1.0, 0.0), |acc, _| acc.mul(z)).sub(a.iter().enumerate().fold(C(0.0, 0.0), |acc, (i, &c)| {
        let mut t = c;
        for _ in 0..p - 1 - i {
            t = t.mul(z);
        }
        acc.add(t)
    }));
    if p == 1 {
        return vec![a[0]];
    }
    if p == 2 {
        let d = a[0].mul(a[0]).add(C(4.0, 0.0).mul(a[1])).sqrt();
        return vec![a[0].add(d).mul(C(0.5, 0.0)), a[0].sub(d).mul(C(0.5, 0.0))];
    }
    let mut z: Vec<C> = (0..p).map(|k| { let t = 0.4 + 0.9 * k as f64; C(0.9 * t.cos(), 0.9 * t.sin()) }).collect();
    for _ in 0..500 {
        for i in 0..p {
            let mut den = C(1.0, 0.0);
            for j in 0..p {
                if j != i {
                    den = den.mul(z[i].sub(z[j]));
                }
            }
            z[i] = z[i].sub(poly(z[i]).div(den));
        }
    }
    z
}

/// M2: the modes of each partial of keys held 5 s (TRAUM_DIR's M2_LABEL,
/// default held5-v60), the reference AU's and ours. Each partial's complex
/// envelope (the mean of the two channels heterodyned to the partial, two
/// 40 ms boxcars, every 40 ms from 0.1 to 4.9 s) is fitted with as many
/// decaying modes as the key has strings (M2_EXTRA more) by linear
/// prediction; per mode its decay in dB/s, its offset from the partial in
/// Hz and its level in dB re the envelope at 0.1 s, loudest first.
fn section_m2() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let label = std::env::var("M2_LABEL").unwrap_or("held5-v60".into());
    let extra: usize = std::env::var("M2_EXTRA").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&dir.join("out").join(format!("{label}.B.wav")))) else {
        println!("{label}: missing");
        return;
    };
    let mut wg = stock(true);
    let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
    println!("== M2 {label}: per partial, modes as decay dB/s, offset Hz, level dB re 0.1 s (reference, then ours)");
    let partials: Vec<usize> = std::env::var("M2_PARTIALS").ok().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect()).unwrap_or(vec![1, 2, 4, 8, 12, 16, 20]);
    for &(t0, ref e) in &script {
        let Play::On(key, _) = *e else { continue };
        let k = wg.scale()[(key - 21) as usize].clone();
        let p = makepad_piano_waveguide::scale::strings_of(key) + extra;
        let tracked = track_partials(sect(&rl, t0 - 0.04, t0 + 4.9), sect(&rr, t0 - 0.04, t0 + 4.9), key, 8000.0);
        println!("{} ({} strings)", name(key), p - extra);
        for &n in &partials {
            // the reference's envelope in dB re 0.1 s, every 0.2 s, for the shape distance
            let mut reference: Option<Vec<f64>> = None;
            for (side, l, r, f) in [("R", &rl, &rr, tracked.iter().find(|q| q.0 == n).map(|q| q.1)), ("W", &wl, &wr, Some(string_partial(k.f1, k.b, n)))] {
                let Some(f) = f else { continue };
                let x: Vec<f64> = sect(l, t0, t0 + 5.0).iter().zip(sect(r, t0, t0 + 5.0)).map(|(a, b)| 0.5 * (*a as f64 + *b as f64)).collect();
                let w = std::f64::consts::TAU * f / fs();
                let mut sum = vec![C(0.0, 0.0); x.len() + 1];
                for (i, &v) in x.iter().enumerate() {
                    sum[i + 1] = sum[i].add(C(v * (w * i as f64).cos(), -v * (w * i as f64).sin()));
                }
                let len = (0.04 * fs()) as usize;
                let box1: Vec<C> = (0..x.len().saturating_sub(len)).map(|i| sum[i + len].sub(sum[i]).mul(C(1.0 / len as f64, 0.0))).collect();
                let mut sum2 = vec![C(0.0, 0.0); box1.len() + 1];
                for (i, &v) in box1.iter().enumerate() {
                    sum2[i + 1] = sum2[i].add(v);
                }
                // centred: the two boxcars delay by one boxcar
                let env: Vec<C> = (0..120).map(|j| ((0.1 + 0.04 * j as f64) * fs()) as usize).filter(|&c| c >= len && c < box1.len()).map(|c| sum2[c].sub(sum2[c - len]).mul(C(1.0 / len as f64, 0.0))).collect();
                if env.len() < 3 * p + 2 {
                    continue;
                }
                // Robust to the fit: the prompt (0.1-0.5 s) and the tail (2.5-4.5 s),
                // each its slope in dB/s and the frequency its phase turns at, and
                // the tail's level re 0.1 s: two stages show a tail well under the
                // start, slower than the prompt, at its own frequency.
                let span = |a: f64, b: f64| -> (f64, f64, f64) {
                    let idx: Vec<usize> = (0..env.len()).filter(|&j| { let t = 0.1 + 0.04 * j as f64; t >= a && t <= b }).collect();
                    let t: Vec<f64> = idx.iter().map(|&j| 0.1 + 0.04 * j as f64).collect();
                    let lv: Vec<f64> = idx.iter().map(|&j| db(env[j].abs().powi(2))).collect();
                    let mut ph: Vec<f64> = idx.iter().map(|&j| env[j].1.atan2(env[j].0)).collect();
                    for j in 1..ph.len() {
                        while ph[j] - ph[j - 1] > std::f64::consts::PI { ph[j] -= std::f64::consts::TAU; }
                        while ph[j] - ph[j - 1] < -std::f64::consts::PI { ph[j] += std::f64::consts::TAU; }
                    }
                    (-slope(&t, &lv), slope(&t, &ph) / std::f64::consts::TAU, lv.iter().sum::<f64>() / lv.len() as f64)
                };
                let shape: Vec<f64> = env.iter().step_by(5).map(|c| db(c.abs().powi(2)) - db(env[0].abs().powi(2))).collect();
                if side == "R" {
                    reference = Some(shape.clone());
                } else if let Some(r) = &reference {
                    let d: Vec<f64> = shape.iter().zip(r).map(|(w, r)| (w - r).abs()).collect();
                    println!("  n{n:<2} shape distance {:5.1} dB (mean |ours - reference|, 0.1-4.9 s)", d.iter().sum::<f64>() / d.len() as f64);
                }
                if std::env::var("M2_DUMP").is_ok_and(|v| v == format!("{key}")) {
                    println!("  n{n:<2} {side} dump {}", env.iter().step_by(5).map(|c| format!("{:4.0}", db(c.abs().powi(2)))).collect::<String>());
                }
                let (pr, pf, _) = span(0.1, 0.5);
                let (tr, tf, tl) = span(2.5, 4.5);
                let start = db(env[0].abs().powi(2));
                println!("  n{n:<2} {side} stages: prompt {pr:5.1} dB/s | tail {:6.1} dB at {tr:5.1} dB/s | tail {:+5.2} Hz ({:+5.2} cents)", tl - start, tf - pf, 1200.0 * ((f + tf - pf) / f).log2());
                let rows: Vec<Vec<C>> = (p..env.len()).map(|i| (1..=p).map(|j| env[i - j]).collect()).collect();
                let a = complex_lstsq(&rows, &env[p..]);
                let roots = lp_roots(&a);
                let vand: Vec<Vec<C>> = (0..env.len()).map(|i| roots.iter().map(|&z| { let mut t = C(1.0, 0.0); for _ in 0..i { t = t.mul(z); } t }).collect()).collect();
                let amps = complex_lstsq(&vand, &env);
                let start = env[0].abs();
                let mut modes: Vec<(f64, f64, f64)> = roots.iter().zip(&amps).map(|(z, c)| (-20.0 * z.abs().log10() / 0.04, z.1.atan2(z.0) / (std::f64::consts::TAU * 0.04), db(c.abs() * c.abs() / (start * start)))).collect();
                modes.sort_by(|a, b| b.2.total_cmp(&a.2));
                let fit: f64 = {
                    let err: f64 = env.iter().zip(&vand).map(|(y, row)| row.iter().zip(&amps).fold(C(0.0, 0.0), |s, (v, c)| s.add(v.mul(*c))).sub(*y).abs().powi(2)).sum();
                    let all: f64 = env.iter().map(|y| y.abs().powi(2)).sum();
                    db(err / all)
                };
                // modes far off the partial are what the extra order takes up of noise and leakage
                let shown: Vec<String> = modes.iter().filter(|m| m.1.abs() < 3.0).map(|m| format!("{:6.1} dB/s {:+5.2} Hz {:6.1} dB", m.0, m.1, m.2)).collect();
                println!("  n{n:<2} {side} {:6.0} Hz | {} | fit {:5.1} dB", f, shown.join(" | "), fit);
            }
        }
    }
}

/// Keys of a held-note run (each key with its onset in s) that the
/// reference AU left silent: their level over the first second more than
/// 30 dB under the run's median. It withholds some keys at every velocity,
/// even alone; measured there, a figure is only the tail of the key before.
fn silent_keys(l: &[f32], r: &[f32], keys: impl Iterator<Item = (u8, f64)>) -> Vec<u8> {
    let levels: Vec<(u8, f64)> = keys.map(|(key, t0)| (key, db(energy(sect(l, t0, t0 + 1.0), sect(r, t0, t0 + 1.0))))).collect();
    let mut sorted: Vec<f64> = levels.iter().map(|k| k.1).collect();
    sorted.sort_by(f64::total_cmp);
    let median = sorted[sorted.len() / 2];
    levels.into_iter().filter(|k| k.1 < median - 30.0).map(|k| k.0).collect()
}

fn section_bassdecay() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let mut wg = variant(&std::env::var("TREBLE_VARIANT").unwrap_or("stock".into()), true);
    println!("== decays at v60, dB/s over 0.1-1.5 s: partials 1-4 | 10-20 | 21-40, R then W, and W - R");
    let set = std::env::var("DECAY_SET").unwrap_or_default();
    // DECAY_TREBLE: the treble run at another velocity (treble-v87, ...)
    let treble = std::env::var("DECAY_TREBLE").unwrap_or("treble-v60".into());
    for (label, first, spacing, keys) in [("bass-v60", 21u8, 3.0, 21u8..=59), (treble.as_str(), 60, 2.5, 60..=108)] {
    if !set.is_empty() && !label.starts_with(set.as_str()) {
        continue;
    }
    let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&dir.join("out").join(format!("{label}.B.wav")))) else {
        println!("{label}: missing");
        continue;
    };
    let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
    let silent = silent_keys(&rl, &rr, keys.clone().map(|key| (key, 0.05 + spacing * (key - first) as f64)));
    for key in keys {
        let t0 = 0.05 + spacing * (key - first) as f64;
        if silent.contains(&key) {
            println!("{:>4} | the reference is silent", name(key));
            continue;
        }
        let means = |l: &[f32], r: &[f32], partials: Vec<(usize, f64)>| {
            let (l, r) = (sect(l, t0 - 0.04, t0 + spacing - 0.1), sect(r, t0 - 0.04, t0 + spacing - 0.1));
            // DECAY_CHANNEL=left or right: measure one channel alone
            let (l, r) = match std::env::var("DECAY_CHANNEL").as_deref() {
                Ok("left") => (l, l),
                Ok("right") => (r, r),
                _ => (l, r),
            };
            let o = onset(l, r);
            let (l, r) = (&l[o..], &r[o..]);
            let f0 = partials[0].1;
            let within: Option<f64> = std::env::var("DECAY_WITHIN").ok().and_then(|v| v.parse().ok());
            let measured: Vec<(f64, f64)> = partials.iter().map(|&(_, f)| match within {
                Some(range) => band_decay_within(l, r, f, 0.4 * f0, range),
                None => band_decay(l, r, f, 0.4 * f0),
            }).collect();
            // beating: rms of each partial's level about its straight decay
            let ripple: Vec<f64> = partials.iter().map(|&(_, f)| {
                let times: Vec<f64> = (0..13).map(|i| 0.1 + 0.1 * i as f64).collect();
                let level: Vec<f64> = times.iter().map(|&t| db(band(&spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2)), f - 0.4 * f0, f + 0.4 * f0))).collect();
                let k = slope(&times, &level);
                let (mt, ml) = (times.iter().sum::<f64>() / 13.0, level.iter().sum::<f64>() / 13.0);
                (times.iter().zip(&level).map(|(t, v)| (v - ml - k * (t - mt)).powi(2)).sum::<f64>() / 13.0).sqrt()
            }).collect();
            let decays: Vec<f64> = measured.iter().map(|m| m.0).collect();
            let starts: Vec<f64> = measured.iter().map(|m| m.1).collect();
            let top = starts.iter().take(4).cloned().fold(f64::MIN, f64::max);
            let mean = |v: &[f64], a: usize, z: usize| v.get(a - 1..z.min(v.len())).map_or(f64::NAN, |d| d.iter().sum::<f64>() / d.len().max(1) as f64);
            if std::env::var("DECAY_TRACE").is_ok_and(|v| v.split(',').any(|k| k.parse::<u8>().ok() == Some(key))) {
                for &(n, f) in partials.iter().filter(|p| [1usize, 2, 3, 4, 8, 12, 16, 20, 30].contains(&p.0)) {
                    let level: Vec<String> = (0..26).map(|i| 0.1 * i as f64).map(|t| format!("{:4.0}", db(band(&spectrum(sect(l, t, t + 0.1), sect(r, t, t + 0.1)), f - 0.4 * f0, f + 0.4 * f0)))).collect();
                    eprintln!("trace {} n{n:<2} {}", name(key), level.join(""));
                }
            }
            if std::env::var("DECAY_M1").is_ok() {
                // Per partial 8-20: the slope over 0.1-0.6 s against 0.9-1.5 s (a
                // knee: late well under early; a slide into a null: late over
                // early; free beating: alike), its dips of 3 dB or more, and how far
                // the left and right channels' envelopes differ (dB rms about
                // their mean difference; string interference is alike in both).
                let env = |a: &[f32], b: &[f32], f: f64, step: f64, len: f64, count: usize| -> Vec<f64> {
                    (0..count).map(|i| 0.1 + step * i as f64).map(|t| db(band(&spectrum(sect(a, t, t + len), sect(b, t, t + len)), f - 0.4 * f0, f + 0.4 * f0))).collect()
                };
                let t: Vec<f64> = (0..13).map(|i| 0.1 + 0.1 * i as f64).collect();
                let (mut kind, mut early, mut late, mut dips, mut lr) = ([0usize; 3], Vec::new(), Vec::new(), Vec::new(), Vec::new());
                for &(_, f) in partials.iter().filter(|p| (8..=20).contains(&p.0)) {
                    let e = env(l, r, f, 0.1, 0.2, 13);
                    let (a, z) = (-slope(&t[..6], &e[..6]), -slope(&t[8..], &e[8..]));
                    kind[if a > 2.0 && z < 0.5 * a { 0 } else if z > a + (0.25 * a.abs()).max(2.0) { 2 } else { 1 }] += 1;
                    early.push(a);
                    late.push(z);
                    let fine = env(l, r, f, 0.05, 0.1, 29);
                    let mut d = 0;
                    for i in 1..fine.len() - 1 {
                        if fine[i] <= fine[i - 1] && fine[i] < fine[i + 1] {
                            let left = fine[i.saturating_sub(5)..i].iter().cloned().fold(f64::MIN, f64::max);
                            let right = fine[i + 1..(i + 6).min(fine.len())].iter().cloned().fold(f64::MIN, f64::max);
                            if left - fine[i] > 3.0 && right - fine[i] > 3.0 {
                                d += 1;
                            }
                        }
                    }
                    dips.push(d as f64);
                    let diff: Vec<f64> = env(l, l, f, 0.1, 0.2, 13).iter().zip(env(r, r, f, 0.1, 0.2, 13)).map(|(x, y)| x - y).collect();
                    let m = diff.iter().sum::<f64>() / diff.len() as f64;
                    lr.push((diff.iter().map(|d| (d - m).powi(2)).sum::<f64>() / diff.len() as f64).sqrt());
                }
                let med = |mut v: Vec<f64>| -> f64 {
                    v.sort_by(f64::total_cmp);
                    v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
                };
                let n = dips.len().max(1) as f64;
                print!("[m1 {}/{}/{} {:.1} {:.1} {:.1} {:.1}] ", kind[0], kind[1], kind[2], med(early), med(late), dips.iter().sum::<f64>() / n, med(lr));
            }
            if std::env::var("DECAY_PARTIALS").is_ok() {
                print!("[n1-6 {}] ", decays.iter().take(6).map(|d| format!(" {d:.1}")).collect::<String>());
            }
            if std::env::var("DECAY_SUMMED").is_ok() {
                // the slope of each band's summed power: beats of its partials average out
                let summed = |a: usize, z: usize| -> f64 {
                    let times: Vec<f64> = (0..13).map(|i| 0.1 + 0.1 * i as f64).collect();
                    let level: Vec<f64> = times.iter().map(|&t| {
                        let sp = spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2));
                        db(partials.iter().filter(|p| p.0 >= a && p.0 <= z).map(|&(_, f)| band(&sp, f - 0.4 * f0, f + 0.4 * f0)).sum::<f64>())
                    }).collect();
                    -slope(&times, &level)
                };
                return [summed(1, 4), summed(10, 20), summed(21, 40)];
            }
            if std::env::var("DECAY_DETAIL").is_ok() {
                print!("[start re 1-4: {:+5.1} {:+5.1}] [ripple {:4.1} {:4.1} {:4.1}] ", mean(&starts, 10, 20) - top, mean(&starts, 21, 40) - top, mean(&ripple, 1, 4), mean(&ripple, 10, 20), mean(&ripple, 21, 40));
            }
            [mean(&decays, 1, 4), mean(&decays, 10, 20), mean(&decays, 21, 40)]
        };
        let tracked = track_partials(sect(&rl, t0 - 0.04, t0 + spacing - 0.1), sect(&rr, t0 - 0.04, t0 + spacing - 0.1), key, 8000.0);
        let r = means(&rl, &rr, tracked);
        let k = wg.scale()[(key - 21) as usize].clone();
        let w = means(&wl, &wr, (1..=60).map(|n| (n, string_partial(k.f1, k.b, n))).take_while(|p| p.1 < 8000.0).collect());
        let level = |l: &[f32], r: &[f32], len: f64| db(energy(sect(l, t0, t0 + len), sect(r, t0, t0 + len)) / len);
        println!(
            "{:>4} | R {:5.1} {:5.1} {:5.1} | W {:5.1} {:5.1} {:5.1} | W-R {:+5.1} {:+5.1} {:+5.1} | level R {:6.1} W {:6.1} | early R {:6.1} W {:6.1}",
            name(key), r[0], r[1], r[2], w[0], w[1], w[2], w[0] - r[0], w[1] - r[1], w[2] - r[2],
            level(&rl, &rr, 1.0), level(&wl, &wr, 1.0), level(&rl, &rr, 0.3), level(&wl, &wr, 0.3)
        );
    }
    }
}

/// A/B of the touch: a treble and a bass note (TOUCH_AB_KEYS, default A5
/// and A1), each at v20/40/60/87/110/127 from soft to hard; ours dry, the
/// reference's cut from its renders of the same notes (TRAUM_DIR,
/// `{treble,bass}-v{vel}`). Each file is under one gain, matched in
/// loudness as a whole, so the steps from blow to blow stay as played;
/// each blow's level over 0-0.5 s against its v60 blow is printed.
/// AB_DIR, AB_TAG.
fn section_touchab() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let ab = std::path::PathBuf::from(std::env::var_os("AB_DIR").expect("AB_DIR"));
    std::fs::create_dir_all(&ab).expect("AB_DIR");
    let tag = std::env::var("AB_TAG").unwrap_or("ours".into());
    let keys: Vec<u8> = std::env::var("TOUCH_AB_KEYS").map_or(vec![81, 33], |v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    let vels = [20u8, 40, 60, 87, 110, 127];
    let mut wg = stock(true);
    let (mut ours, mut theirs) = ((Vec::new(), Vec::new()), (Vec::new(), Vec::new()));
    for &key in &keys {
        let (set, first, every, hold) = if key >= 60 { ("treble", 60u8, 2.5, 1.5) } else { ("bass", 21u8, 3.0, 2.0) };
        let (mut line, mut levels) = (format!("{:>4} |", name(key)), [[0.0f64; 6]; 2]);
        for (i, &vel) in vels.iter().enumerate() {
            let Some((rl, rr, _)) = read_wav(&dir.join("out").join(format!("{set}-v{vel}.B.wav"))) else {
                println!("{set}-v{vel}: missing");
                return;
            };
            let t0 = 0.05 + every * (key - first) as f64;
            let (rl, rr) = (sect(&rl, t0 - 0.05, t0 - 0.05 + every), sect(&rr, t0 - 0.05, t0 - 0.05 + every));
            wg.reset();
            let (wl, wr) = render(&mut wg, &[(0.05, PianoEvent::NoteOn { key, velocity: vel }), (0.05 + hold, PianoEvent::NoteOff { key })], every);
            for (side, (l, r)) in [(&wl[..], &wr[..]), (rl, rr)].into_iter().enumerate() {
                let o = onset(l, r);
                levels[side][i] = db(energy(sect(&l[o..], 0.0, 0.5), sect(&r[o..], 0.0, 0.5)));
            }
            ours.0.extend_from_slice(&wl);
            ours.1.extend_from_slice(&wr);
            theirs.0.extend_from_slice(rl);
            theirs.1.extend_from_slice(rr);
        }
        for (i, &vel) in vels.iter().enumerate() {
            line += &format!(" v{vel} {:+5.1}/{:+5.1}", levels[0][i] - levels[0][2], levels[1][i] - levels[1][2]);
        }
        println!("{line}   (dB against v60: ours/reference)");
    }
    let pair = [(tag.as_str(), ours), ("reference", theirs)];
    let gains: Vec<f64> = pair.iter().map(|(_, (l, r))| (1e-2 / k_loudness(l, r)).sqrt()).collect();
    let peak = pair.iter().zip(&gains).map(|((_, (l, r)), g)| l.iter().chain(r.iter()).fold(0.0f64, |m, &x| m.max(x.abs() as f64)) * g).fold(0.0, f64::max);
    let trim = (10f64.powf(-1.0 / 20.0) / peak).min(1.0);
    for ((name, (l, r)), g) in pair.iter().zip(&gains) {
        let g = (g * trim) as f32;
        write_wav(&ab.join(format!("touch-{name}.wav")), &l.iter().map(|x| x * g).collect::<Vec<_>>(), &r.iter().map(|x| x * g).collect::<Vec<_>>());
        println!("touch-{name}: gain {:+.1} dB", 20.0 * (g as f64).log10());
    }
}

/// The top octave key by key (TOP_KEYS, F#7-C8 by default): the loop as it
/// would be at the output's rate (delay line in samples besides its
/// filters, dispersion sections, and the strike comb's span in samples,
/// before it is held at one), then per blow (TOP_VELS, v40 v60 v110) over
/// its first 50 ms partial 1 at the bridge in dB, partials 2 and 3 against
/// it, partial 2 against 1 of the hammer's force, and the first contact in
/// periods.
fn section_topoctave() {
    let keys: Vec<u8> = std::env::var("TOP_KEYS").ok().map_or((102u8..=108).collect(), |v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    let mut p = stock(true);
    let scale = p.scale().to_vec();
    let vels: Vec<u8> = std::env::var("TOP_VELS").ok().map_or(vec![40, 60, 110], |v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    println!("== topoctave at {} Hz: key | delay sections span at this rate | per velocity ({vels:?}): bridge P1 dB, P2 P3 against it, force P2 against its P1, first contact in periods", fs());
    for &key in &keys {
        let k = &scale[(key - 21) as usize];
        let f = makepad_piano_waveguide::loop_filters(k, fs());
        let t = tune(&f, k.f1, fs());
        let n_null = 1.0 / k.strike;
        let f_null = k.f1 / (1.0 + k.b).sqrt() * n_null * (1.0 + k.b * n_null * n_null).sqrt();
        let mut line = format!("top {:>4} | {:2} {:2} {:.2} |", name(key), t.delay, f.sections, fs() / f_null);
        for &vel in &vels {
            let (x, force) = p.blow_trace(key, vel, (0.05 * fs()) as usize);
            // first contact from the force trace: from its first push to its first rest
            let on = force.iter().position(|&f| f > 0.0).unwrap_or(0);
            let blow = force[on..].iter().position(|&f| f <= 0.0).unwrap_or(0) as f64 / fs() * k.f1;
            let s = spectrum(&x, &x);
            let part = |n: usize| db(partial_power(&s, k.f1, k.b, n));
            let sf = spectrum(&force, &force);
            let fpart = |n: usize| db(partial_power(&sf, k.f1, k.b, n));
            line += &format!(" {:6.1} {:+6.1} {:+6.1} {:+6.1} {:.2} |", part(1), part(2) - part(1), part(3) - part(1), fpart(2) - fpart(1), blow);
        }
        println!("{line}");
    }
}

/// The A/B of a chromatic run: each `chrom-v<vel>` script under TRAUM_DIR
/// rendered here and set beside the reference AU's render of it, both at a
/// K-weighted loudness of -20 dB and turned down together if either would
/// pass -1 dBFS, into AB_DIR as `chrom-v<vel>-ours.wav` and `-reference.wav`
/// (other scripts and names by AB_LABELS and AB_TAG, cut to AB_FROM..AB_UNTIL s).
fn section_chrom() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let ab = std::path::PathBuf::from(std::env::var_os("AB_DIR").expect("AB_DIR"));
    std::fs::create_dir_all(&ab).expect("AB_DIR");
    let mut wg = stock(true);
    // AB_LABELS: the scripts (default the two chromatic runs); AB_TAG names
    // our file (default ours); AB_UNTIL cuts the passage, s.
    let labels = std::env::var("AB_LABELS").unwrap_or("chrom-v60,chrom-v100".into());
    let tag = std::env::var("AB_TAG").unwrap_or("ours".into());
    let until: Option<f64> = std::env::var("AB_UNTIL").ok().and_then(|v| v.parse().ok());
    let from: f64 = std::env::var("AB_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    for label in labels.split(',') {
        let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&dir.join("out").join(format!("{label}.B.wav")))) else {
            println!("{label}: missing");
            continue;
        };
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        let cut = |x: Vec<f32>| -> Vec<f32> {
            let end = until.map_or(x.len(), |u| ((u * fs()) as usize).min(x.len()));
            x[((from * fs()) as usize).min(end)..end].to_vec()
        };
        let pair = [(tag.as_str(), cut(wl), cut(wr)), ("reference", cut(rl), cut(rr))];
        let gains: Vec<f64> = pair.iter().map(|(_, l, r)| (1e-2 / k_loudness(l, r)).sqrt()).collect();
        let peak = pair.iter().zip(&gains).map(|((_, l, r), g)| l.iter().chain(r.iter()).fold(0.0f64, |m, &x| m.max(x.abs() as f64)) * g).fold(0.0, f64::max);
        let trim = (10f64.powf(-1.0 / 20.0) / peak).min(1.0);
        for ((name, l, r), g) in pair.iter().zip(&gains) {
            let g = (g * trim) as f32;
            write_wav(&ab.join(format!("{label}-{name}.wav")), &l.iter().map(|x| x * g).collect::<Vec<_>>(), &r.iter().map(|x| x * g).collect::<Vec<_>>());
            println!("{label}-{name}: gain {:+.1} dB", 20.0 * (g as f64).log10());
        }
    }
}

/// BS.1770 K-weighting (a high shelf of about +4 dB above 1.5 kHz and a
/// high-pass near 38 Hz) at the current rate: the mean square of both
/// channels after it, summed, as a loudness measure.
fn k_loudness(l: &[f32], r: &[f32]) -> f64 {
    let rate = fs();
    let shelf = {
        let (g, f0, q) = (3.999843853973347f64, 1681.974450955533f64, 0.7071752369554196f64);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let vh = 10f64.powf(g / 20.0);
        let vb = vh.powf(0.4996667741545416);
        let a0 = 1.0 + k / q + k * k;
        [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]
    };
    let high = {
        let (f0, q) = (38.13547087602444f64, 0.5003270373238773f64);
        let k = (std::f64::consts::PI * f0 / rate).tan();
        let a0 = 1.0 + k / q + k * k;
        [1.0, -2.0, 1.0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0]
    };
    let mut sum = 0.0;
    for x in [l, r] {
        let mut st = [[0.0f64; 4]; 2];
        let mut ms = 0.0;
        for &v in x {
            let mut y = v as f64;
            for (c, s) in [shelf, high].iter().zip(st.iter_mut()) {
                // direct form I
                let out = c[0] * y + c[1] * s[0] + c[2] * s[1] - c[3] * s[2] - c[4] * s[3];
                s[1] = s[0];
                s[0] = y;
                s[3] = s[2];
                s[2] = out;
                y = out;
            }
            ms += y * y;
        }
        sum += ms / x.len().max(1) as f64;
    }
    sum
}

/// 24-bit PCM stereo WAV.
/// Frequency of the strongest peak within 6 % of `near`: Hann-windowed
/// spectrum, parabolic interpolation on the log power.
fn peak_freq(l: &[f32], r: &[f32], near: f64) -> f64 {
    let s = spectrum(l, r);
    let (a, b) = (((0.94 * near / s.0) as usize).max(1), ((1.06 * near / s.0) as usize).min(s.1.len() - 2));
    let k = (a..=b).max_by(|&i, &j| s.1[i].total_cmp(&s.1[j])).unwrap_or(a);
    let (y0, y1, y2) = (s.1[k - 1].max(1e-30).ln(), s.1[k].max(1e-30).ln(), s.1[k + 1].max(1e-30).ln());
    let d = 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2);
    (k as f64 + if d.is_finite() { d.clamp(-0.5, 0.5) } else { 0.0 }) * s.0
}

/// Each value less the median of the values within `reach` keys of it.
fn off_trend(v: &[f64], reach: usize) -> Vec<f64> {
    (0..v.len())
        .map(|i| {
            let mut near: Vec<f64> = v[i.saturating_sub(reach)..(i + reach + 1).min(v.len())].iter().copied().filter(|x| x.is_finite()).collect();
            near.sort_by(f64::total_cmp);
            v[i] - near.get(near.len() / 2).copied().unwrap_or(f64::NAN)
        })
        .collect()
}

fn rms_of(v: &[f64]) -> f64 {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    (f.iter().map(|x| x * x).sum::<f64>() / f.len().max(1) as f64).sqrt()
}

/// One key's design: f1, B, Z per string, strings, length m, bridge R, b1,
/// b3, allpass sections, worst partial error (cents) to n=16/32/64, hammer
/// g / p / contact target ms, longitudinal Hz, bridge spikes v60/v87.
fn design_row(wg: &mut WaveguidePiano, key: u8) -> String {
    let k = wg.scale()[(key - 21) as usize].clone();
    let f = makepad_piano_waveguide::loop_filters(&k, fs());
    let t = tune(&f, k.f1, fs());
    let mut worst = [0.0f64; 3];
    for n in 1..=64usize {
        let want = string_partial(k.f1, k.b, n);
        if want > 12_000.0 {
            break;
        }
        let cents = (1200.0 * (loop_partial(&f, t, n, fs()) / want).log2()).abs();
        for (i, lim) in [16usize, 32, 64].into_iter().enumerate() {
            if n <= lim {
                worst[i] = worst[i].max(cents);
            }
        }
    }
    let spikes = [60u8, 87].map(|v| find_clicks(&wg.blow_trace(key, v, (0.25 * fs()) as usize).0, 20.0).len());
    format!("{:>3} f1 {:6.2} B {:.2e} Z {:4.2} x{} L {:.2} R {:6.0} b1 {:.2} b3 {:.1e} sections {:2} | cents {:5.2} {:5.2} {:5.2} | hammer {:.1} g p {:.2} {:.2} ms | long {:5.0} Hz | spikes {:?}",
        name(key), k.f1, k.b, k.z, k.strings, k.length, k.bridge_r, k.b1, k.b3, f.sections, worst[0], worst[1], worst[2], k.hammer_mass * 1e3, k.hammer_p, k.contact * 1e3, k.longitudinal, spikes)
}

/// Where a treble key's sustained treble comes from, at the bridge alone:
/// per key at v60, partial 1's level early (0.05-0.25 s) and its decay, the
/// partials above 4 kHz together early and late (1.1-1.3 s), and the
/// slowest of them, each beside its distance from the neighbours' trend.
fn section_decays() {
    let mut wg = stock(true);
    let keys: Vec<u8> = (72u8..=102).collect();
    let len = (1.35 * fs()) as usize;
    let amp = |x: &[f32], f: f64, from: f64, to: f64| {
        let x = &x[(from * fs()) as usize..(to * fs()) as usize];
        let (step_im, step_re) = (std::f64::consts::TAU * f / fs()).sin_cos();
        let (mut c, mut s, mut re, mut im) = (1.0, 0.0, 0.0, 0.0);
        for (i, &v) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
            re += w * v as f64 * c;
            im += w * v as f64 * s;
            (c, s) = (c * step_re - s * step_im, s * step_re + c * step_im);
        }
        (re * re + im * im).sqrt() / x.len() as f64
    };
    if let Ok(list) = std::env::var("PEAKS") {
        // The strongest spectral peaks above 3 kHz, early and late, against
        // the string's partials: Hz, dB re partial 1 early, nearest n, cents.
        for key in list.split(',').filter_map(|k| k.parse::<u8>().ok()) {
            let k = wg.scale()[(key - 21) as usize].clone();
            let (bridge, _) = wg.blow_trace(key, 60, len);
            let ref1 = db(amp(&bridge, k.f1, 0.05, 0.25).powi(2));
            for (from, to) in [(0.05, 0.25), (1.1, 1.3)] {
                let bins: Vec<(f64, f64)> = (0..((0.45 * fs() - 3000.0) / 2.5) as usize).map(|i| 3000.0 + 2.5 * i as f64).map(|f| (f, db(amp(&bridge, f, from, to).powi(2)) - ref1)).collect();
                let mut peaks: Vec<(f64, f64)> = (1..bins.len() - 1).filter(|&i| bins[i].1 > bins[i - 1].1 && bins[i].1 >= bins[i + 1].1).map(|i| bins[i]).collect();
                peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
                let text: Vec<String> = peaks.iter().take(8).map(|&(f, l)| {
                    let n = (1..=60).min_by(|&a, &b| (string_partial(k.f1, k.b, a) - f).abs().total_cmp(&(string_partial(k.f1, k.b, b) - f).abs())).unwrap();
                    format!("{f:.0} {l:+.0} n{n} {:+.0}c", 1200.0 * (f / string_partial(k.f1, k.b, n)).log2())
                }).collect();
                println!("{:>4} {from:.2}-{to:.2}s: {}", name(key), text.join(" | "));
            }
        }
        return;
    }
    let mut rows = Vec::new();
    for &key in &keys {
        let k = wg.scale()[(key - 21) as usize].clone();
        let (bridge, _) = wg.blow_trace(key, 60, len);
        let p1 = (db(amp(&bridge, k.f1, 0.05, 0.25).powi(2)), db(amp(&bridge, k.f1, 1.1, 1.3).powi(2)));
        let (mut early, mut late, mut slowest, mut slow_n) = (0.0, 0.0, f64::MAX, 0);
        for n in 1..=40 {
            let f = string_partial(k.f1, k.b, n);
            if f < 4000.0 {
                continue;
            }
            if f > 0.45 * fs() {
                break;
            }
            let (a, b) = (amp(&bridge, f, 0.05, 0.25).powi(2), amp(&bridge, f, 1.1, 1.3).powi(2));
            early += a;
            late += b;
            let decay = (db(a) - db(b)) / 1.05;
            if decay < slowest {
                slowest = decay;
                slow_n = n;
            }
        }
        rows.push((key, p1.0, (p1.0 - p1.1) / 1.05, db(early), db(late), slowest, slow_n));
    }
    let col = |f: &dyn Fn(&(u8, f64, f64, f64, f64, f64, usize)) -> f64| off_trend(&rows.iter().map(f).collect::<Vec<_>>(), 2);
    let (o1, od, oe, ol) = (col(&|r| r.1), col(&|r| r.2), col(&|r| r.3 - r.1), col(&|r| r.4 - (r.1 - 1.15 * r.2)));
    println!("== decays at the bridge, v60: p1 dB (off), p1 decay dB/s (off) | >4k early re p1 (off), >4k late re p1 late (off) | slowest >4k partial dB/s");
    for (i, r) in rows.iter().enumerate() {
        println!("{:>4} p1 {:+6.1} ({:+5.1}) decay {:5.1} ({:+5.1}) | hf early {:+6.1} ({:+5.1}) late {:+6.1} ({:+5.1}) | slowest n{} {:5.1} dB/s",
            name(r.0), r.1, o1[i], r.2, od[i], r.3 - r.1, oe[i], r.4 - (r.1 - 1.15 * r.2), ol[i], r.6, r.5);
    }
}

/// Our tuning against the reference AU's, key by key at v60: the frequency of
/// one partial of each key (the 1st from A2 up, the 2nd from A1, the 4th
/// below), measured the same way on both, W against R in cents, and each
/// one's stretch from equal temperament. Reads bass-v60 (A0-B3, 3 s apart)
/// and treble-v60 (C4-C8, 2.5 s apart) under TRAUM_DIR.
fn section_stretch() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let out = dir.join("out");
    let mut wg = stock(true);
    println!("== stretch at v60: key, partial, R and W in cents from equal temperament (times n), W - R");
    for (label, first, spacing) in [("bass-v60", 21u8, 3.0), ("treble-v60", 60, 2.5)] {
        let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&out.join(format!("{label}.B.wav")))) else {
            println!("{label}: missing");
            continue;
        };
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        let keys = if first == 21 { 21u8..=59 } else { 60u8..=108 };
        let silent = silent_keys(&rl, &rr, keys.clone().enumerate().map(|(i, key)| (key, 0.05 + spacing * i as f64)));
        for (i, key) in keys.enumerate() {
            let t0 = 0.05 + spacing * i as f64;
            if silent.contains(&key) {
                println!("{:>4} | the reference is silent", name(key));
                continue;
            }
            let n = if key >= 45 { 1.0 } else if key >= 33 { 2.0 } else { 4.0 };
            let et = n * 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
            let at = |l: &[f32], r: &[f32]| {
                let (l, r) = (sect(l, t0 - 0.04, t0 + spacing - 0.1), sect(r, t0 - 0.04, t0 + spacing - 0.1));
                let o = onset(l, r);
                let (l, r) = (&l[o..], &r[o..]);
                let f = peak_freq(sect(l, 0.1, 1.6), sect(r, 0.1, 1.6), et);
                1200.0 * (f / et).log2()
            };
            let (cr, cw) = (at(&rl, &rr), at(&wl, &wr));
            println!("{:>4} n{} | R {:+6.1} W {:+6.1} | W-R {:+5.1}", name(key), n as u32, cr, cw, cw - cr);
        }
    }
}

/// How even the treble is, key by key against the reference AU: C4-C8 each
/// held 1.5 s, 2.5 s apart, at v40/60/87/110 (scripts treble-v<vel>.txt
/// under TRAUM_DIR, the reference's renders in out/). Per key: share above
/// 4 kHz in the first 50 ms (attack) and at 0.3-1.3 s (sustain), dB re the
/// note; the rise to the loudest millisecond; the fundamental, W against R in
/// cents; the level of the first 0.5 s. Then how far each key stands from the
/// median of the two keys either side of it, for both, and the keys where ours
/// stands out and the reference does not.
/// Touch against the reference AU: the bass (A0-B3, a key every 3 s, held
/// 2 s) and the treble (C4-C8, every 2.5 s) at v40, v60, v87 and v110, dry.
/// Per key and side: the level of the first 0.5 s and the share above 4 kHz
/// in the first 50 ms (attack) and 0.05-0.3 s (early). Per register: ours
/// minus the reference at each velocity, how much each grows from layer to
/// layer, and the level's growth; then the step of each from E1 to F1.
/// `FELT_KEYS=1` prints every key. The treble needs treble-v*.B.wav, the
/// bass bass-v*.B.wav in TRAUM_DIR/out.
/// The felt and first contact of the hammers around the bass's breaks.
fn felt_hammers(wg: &mut WaveguidePiano, vels: &[u8]) {
    let felt = wg.felt_at_mm();
    println!("== felt: the hammers at the low breaks: strings, felt N at 1 mm, then per velocity first contact ms / peak N");
    let (from, to) = std::env::var("FELT_HAMMER_KEYS").ok().and_then(|r| r.split_once("..").and_then(|(a, b)| Some((a.parse::<u8>().ok()?, b.parse::<u8>().ok()?)))).unwrap_or((24, 50));
    for key in from..=to {
        let mut line = format!("{:>4} | {} {:6.1} N |", name(key), makepad_piano_waveguide::scale::strings_of(key), felt[(key - 21) as usize]);
        for &vel in vels.iter().chain(&[67u8]) {
            wg.reset();
            render(wg, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 0.05);
            let (contact, force) = wg.last_blow(key);
            line += &format!(" v{vel} {:4.2} / {force:5.1}", contact * 1e3);
        }
        println!("{line}");
    }
}

/// The touch on a grid of felt exponents and contact scales: each anchor
/// key alone, dry, at v40/60/87/110. Per key and grid point: the v60
/// attack share above 4 kHz, its growth from v40 to v110 and the level's
/// growth over 0-0.5 s from v40 to v110 (dB), to set against the
/// reference's from the `felt` section.
fn section_touchfit() {
    let list = |name: &str, default: &str| -> Vec<f64> { std::env::var(name).unwrap_or(default.into()).split(',').filter_map(|x| x.parse().ok()).collect() };
    let (exponents, contacts) = (list("FIT_P", "0.2,0.4,0.6,0.9"), list("FIT_C", "0.8,1.0,1.2"));
    let keys: Vec<u8> = list("FIT_KEYS", "60,64,67,72,76,79,84,88,91,96,100,103,108").iter().map(|&k| k as u8).collect();
    let vels = [40u8, 60, 87, 110];
    println!("== touchfit: key felt_exponent contact | v60 attack >4k, its growth v40->110, level growth v40->110 (dB)");
    for &fe in &exponents {
        for &c in &contacts {
            let (mut v, b) = settings();
            v.felt_exponent = fe;
            v.contact = c;
            let mut p = build(&v, &b, true);
            for &key in &keys {
                let (mut level, mut attack) = ([0.0; 4], [0.0; 4]);
                for (i, &vel) in vels.iter().enumerate() {
                    p.reset();
                    // FIT_BRIDGE: the bridge's own motion, before the board
                    let (l, r) = if std::env::var("FIT_BRIDGE").is_ok() {
                        let v = p.bridge_velocity(key, vel, (0.6 * fs()) as usize);
                        (v.clone(), v)
                    } else {
                        render(&mut p, &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], 0.6)
                    };
                    let o = onset(&l, &r);
                    let (l, r) = (&l[o..], &r[o..]);
                    level[i] = db(energy(sect(l, 0.0, 0.5), sect(r, 0.0, 0.5)));
                    attack[i] = db(soft_share(&spectrum(sect(l, 0.0, 0.05), sect(r, 0.0, 0.05)), 4000.0));
                }
                println!("fit {key} {fe:.2} {c:.2} | {:+.1} {:+.1} {:+.1}", attack[1], attack[3] - attack[0], level[3] - level[0]);
            }
        }
    }
}

/// Where the two grands sit against each other as Score plays them (both
/// dry: Score's room comes after; master 1): K-weighted loudness and peak
/// of the same passages, and the difference.
fn section_engines() {
    let held = |keys: &[u8], vel: u8| -> (Vec<(f64, Play)>, f64) {
        let mut script = Vec::new();
        for &k in keys {
            script.push((0.05, Play::On(k, vel)));
            script.push((2.55, Play::Off(k)));
        }
        (script, 3.0)
    };
    let mid = [48u8, 52, 55, 60, 64, 67, 72];
    let passages: Vec<(&str, (Vec<(f64, Play)>, f64))> = vec![
        ("phrase (v58-90)", passage_phrase()),
        ("chord C3-C5 v40", held(&mid, 40)),
        ("chord C3-C5 v67", held(&mid, 67)),
        ("chord C3-C5 v87", held(&mid, 87)),
        ("chord C3-C5 v110", held(&mid, 110)),
        ("hands C2-E5 v127 ped", {
            let (mut script, secs) = held(&[36u8, 43, 48, 52, 55, 60, 64, 67, 72, 76], 127);
            script.insert(0, (0.0, Play::Pedal(1.0)));
            (script, secs)
        }),
    ];
    // ENGINES_TRIM_DB: a trim on the waveguide's output, after its limiter;
    // ENGINES_LIMITED=1: the modal grand with its own limiter and knee on,
    // as Score plays it. Also the share of samples over the bus knee (0.78).
    let trim = 10f32.powf(std::env::var("ENGINES_TRIM_DB").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0) / 20.0);
    let limited = std::env::var("ENGINES_LIMITED").is_ok_and(|v| v == "1");
    println!("== engines: K-weighted loudness dB, peak dBFS and % over the bus knee, dry at master 1 (waveguide trim x{trim:.2}, modal limiter {limited}), and the waveguide minus the modal grand");
    let mut players = Player::both(true);
    if let Player::Modal(m) = &mut players[1].1 {
        m.set_soft_clip(limited);
    }
    for (label, (script, secs)) in &passages {
        let mut line = format!("{label:<18}|");
        let mut loud = Vec::new();
        for (name, player) in players.iter_mut() {
            let (mut l, mut r) = player.play(script, *secs);
            if matches!(player, Player::Waveguide(_)) {
                l.iter_mut().chain(r.iter_mut()).for_each(|v| *v *= trim);
            }
            // ENGINES_BUS_LIMIT=1: then Score's bus, room off: the modal
            // grand's own gain rider, then the knee, on either engine.
            if std::env::var("ENGINES_BUS_LIMIT").is_ok_and(|v| v == "1") {
                let mut limiter = makepad_piano_model::fx::Limiter::new(fs() as f32);
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    let (x, y) = limiter.process(*a, *b);
                    (*a, *b) = (makepad_piano_model::fx::soft_clip(x), makepad_piano_model::fx::soft_clip(y));
                }
            }
            let over = 100.0 * l.iter().chain(&r).filter(|v| v.abs() > 0.78).count() as f64 / (2 * l.len()) as f64;
            let k = -0.691 + 10.0 * k_loudness(&l, &r).log10();
            let peak = 20.0 * l.iter().chain(&r).fold(0.0f32, |m, &v| m.max(v.abs())).log10();
            line += &format!(" {name} {k:7.2} / {peak:+6.2} / {over:5.3} % |");
            loud.push(k);
        }
        println!("{line} difference {:+.2} dB", loud[0] - loud[1]);
    }
}

fn section_felt() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let mut wg = variant(&std::env::var("TREBLE_VARIANT").unwrap_or("stock".into()), true);
    let vels: Vec<u8> = std::env::var("FELT_VELS").map_or(vec![40, 60, 87, 110], |v| v.split(',').filter_map(|x| x.parse().ok()).collect());
    if std::env::var("FELT_HAMMERS").is_ok() {
        return felt_hammers(&mut wg, &vels);
    }
    // [vel][side][metric][key 21..108]: level, attack share, early share
    let mut m = vec![vec![vec![vec![f64::NAN; 88]; 3]; 2]; vels.len()];
    for (vi, &vel) in vels.iter().enumerate() {
        for (set, first, last, every) in [("bass", 21u8, 59u8, 3.0), ("treble", 60, 108, 2.5)] {
            if std::env::var("FELT_SETS").is_ok_and(|only| !only.split(',').any(|o| o == set)) {
                continue;
            }
            let label = format!("{set}-v{vel}");
            let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&dir.join("out").join(format!("{label}.B.wav")))) else {
                println!("{label}: missing");
                continue;
            };
            let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
            for key in first..=last {
                let t0 = 0.05 + every * (key - first) as f64;
                for (side, (l, r)) in [(&rl, &rr), (&wl, &wr)].into_iter().enumerate() {
                    let (l, r) = (sect(l, t0 - 0.04, t0 + 1.0), sect(r, t0 - 0.04, t0 + 1.0));
                    let o = onset(l, r);
                    let (l, r) = (&l[o..], &r[o..]);
                    let share = |a: f64, z: f64| db(soft_share(&spectrum(sect(l, a, z), sect(r, a, z)), 4000.0));
                    let k = (key - 21) as usize;
                    m[vi][side][0][k] = db(energy(sect(l, 0.0, 0.5), sect(r, 0.0, 0.5)));
                    m[vi][side][1][k] = share(0.0, 0.05);
                    m[vi][side][2][k] = share(0.05, 0.3);
                }
            }
        }
    }
    // Keys the reference left silent (a render with a note missing) say
    // nothing about either instrument: leave them out on both sides.
    for k in 0..88 {
        if (0..vels.len()).any(|vi| m[vi][0][0][k] < m[vi][1][0][k] - 30.0) {
            for vi in 0..vels.len() {
                for side in 0..2 {
                    for q in 0..3 {
                        m[vi][side][q][k] = f64::NAN;
                    }
                }
            }
        }
    }
    let what = ["level", "attack >4k", "early >4k"];
    if std::env::var("FELT_KEYS").is_ok() {
        felt_hammers(&mut wg, &vels);
        for k in 0..88 {
            let row: Vec<String> = (0..vels.len()).map(|vi| format!("v{} {}", vels[vi], (0..3).map(|q| format!("{:+6.1}/{:+6.1}", m[vi][1][q][k], m[vi][0][q][k])).collect::<Vec<_>>().join(" "))).collect();
            println!("{:>4} | {}", name(k as u8 + 21), row.join(" | "));
        }
    }
    let regions: [(&str, u8, u8); 8] = [("A0-E1", 21, 28), ("F1-B1", 29, 35), ("C2-B2", 36, 47), ("C3-B3", 48, 59), ("C4-B4", 60, 71), ("C5-B5", 72, 83), ("C6-B6", 84, 95), ("C7-C8", 96, 108)];
    let mean = |vi: usize, side: usize, q: usize, a: u8, z: u8| -> f64 {
        let v: Vec<f64> = (a..=z).map(|key| m[vi][side][q][(key - 21) as usize]).filter(|x| x.is_finite()).collect();
        v.iter().sum::<f64>() / v.len().max(1) as f64
    };
    for q in 0..3 {
        let steps: Vec<String> = vels.windows(2).map(|w| format!("v{}->{}", w[0], w[1])).collect();
        println!("== felt: {} dB, ours / reference at {}, then its growth {} (ours / reference)", what[q], vels.iter().map(|v| format!("v{v}")).collect::<Vec<_>>().join(" "), steps.join(", "));
        for &(rn, a, z) in &regions {
            let at: Vec<String> = (0..vels.len()).map(|vi| format!("{:+6.1}/{:+6.1}", mean(vi, 1, q, a, z), mean(vi, 0, q, a, z))).collect();
            let grow: Vec<String> = (1..vels.len()).map(|vi| format!("{:+5.1}/{:+5.1}", mean(vi, 1, q, a, z) - mean(vi - 1, 1, q, a, z), mean(vi, 0, q, a, z) - mean(vi - 1, 0, q, a, z))).collect();
            println!("{rn:<6} | {} | grows {}", at.join(" "), grow.join(" "));
        }
    }
    println!("== felt: the step from E1 to F1 (F1 minus E1), ours / reference");
    for (vi, &vel) in vels.iter().enumerate() {
        let step: Vec<String> = (0..3).map(|q| format!("{} {:+5.1}/{:+5.1}", what[q], m[vi][1][q][8] - m[vi][1][q][7], m[vi][0][q][8] - m[vi][0][q][7])).collect();
        println!("v{vel:<4}| {}", step.join(" | "));
    }
}

fn section_treble() {
    let dir = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR"));
    let out = dir.join("out");
    let mut wg = variant(&std::env::var("TREBLE_VARIANT").unwrap_or("stock".into()), true);
    let keys: Vec<u8> = (60..=108).collect();
    for vel in [40u8, 60, 87, 110] {
        let label = format!("treble-v{vel}");
        let (Some((script, secs)), Some((rl, rr, _))) = (read_script(&dir.join("scripts").join(format!("{label}.txt"))), read_wav(&out.join(format!("{label}.B.wav")))) else {
            println!("{label}: missing");
            continue;
        };
        let (wl, wr, _) = render_score(&mut wg, &script, secs, 512, None);
        write_wav(&out.join(format!("{label}.W.wav")), &wl, &wr);
        // [side][metric][key]: attack share, sustain share, rise ms, f1 Hz, level dB
        let mut m = vec![vec![vec![f64::NAN; keys.len()]; 5]; 2];
        for (i, &key) in keys.iter().enumerate() {
            let t0 = 0.05 + 2.5 * i as f64;
            let nominal = 440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0);
            for (side, (l, r)) in [(&rl, &rr), (&wl, &wr)].into_iter().enumerate() {
                let (l, r) = (sect(l, t0 - 0.04, t0 + 2.4), sect(r, t0 - 0.04, t0 + 2.4));
                let o = onset(l, r);
                let (l, r) = (&l[o..], &r[o..]);
                let share = |a: f64, z: f64| db(soft_share(&spectrum(sect(l, a, z), sect(r, a, z)), 4000.0));
                let ms = (fs() / 1000.0) as usize;
                let env: Vec<f64> = (0..150).map(|j| energy(&l[j * ms..(j + 1) * ms], &r[j * ms..(j + 1) * ms])).collect();
                let rise = env.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map_or(f64::NAN, |p| p.0 as f64);
                m[side][0][i] = share(0.0, 0.05);
                m[side][1][i] = share(0.3, 1.3);
                m[side][2][i] = rise;
                m[side][3][i] = peak_freq(sect(l, 0.1, 0.6), sect(r, 0.1, 0.6), nominal);
                m[side][4][i] = db(energy(sect(l, 0.0, 0.5), sect(r, 0.0, 0.5)));
            }
        }
        // Keys the reference left silent or played as another note: no
        // reference there.
        let mut louds = m[0][4].clone();
        louds.sort_by(f64::total_cmp);
        let loud = louds[louds.len() / 2];
        let mut lost = Vec::new();
        for i in 0..keys.len() {
            let nominal = 440.0 * 2.0f64.powf((keys[i] as f64 - 69.0) / 12.0);
            if m[0][4][i] < loud - 30.0 || (1200.0 * (m[0][3][i] / nominal).log2()).abs() > 50.0 {
                lost.push(name(keys[i]));
                for metric in m[0].iter_mut() {
                    metric[i] = f64::NAN;
                }
            }
        }
        println!("{label}: no reference for {lost:?}");
        let cents: Vec<f64> = (0..keys.len()).map(|i| 1200.0 * (m[1][3][i] / m[0][3][i]).log2()).collect();
        // ours as built (its partial 1) and as heard, and the reference against equal temperament
        let built: Vec<f64> = keys.iter().enumerate().map(|(i, &key)| 1200.0 * (m[1][3][i] / wg.scale()[(key - 21) as usize].f1).log2()).collect();
        let stretch: Vec<f64> = keys.iter().enumerate().map(|(i, &key)| 1200.0 * (m[0][3][i] / (440.0 * 2.0f64.powf((key as f64 - 69.0) / 12.0))).log2()).collect();
        let level: Vec<f64> = (0..keys.len()).map(|i| m[1][4][i] - m[0][4][i]).collect();
        let mut sorted = level.clone();
        sorted.sort_by(f64::total_cmp);
        let level: Vec<f64> = level.iter().map(|x| x - sorted[sorted.len() / 2]).collect();
        let off: Vec<Vec<Vec<f64>>> = (0..2).map(|side| [0, 1, 2, 4].iter().map(|&k| off_trend(&m[side][k], 2)).collect()).collect();
        let cents_off = off_trend(&cents, 2);
        let stretch_off = off_trend(&stretch, 2);
        println!("== {label}: attack / sustain share above 4 kHz dB, rise ms, W-R cents, W-R level dB (less its median), and off the trend of the 2 keys either side (W / R)");
        for (i, &key) in keys.iter().enumerate() {
            println!("{:>4} v{vel} | atk W {:+6.1} R {:+6.1} | sus W {:+6.1} R {:+6.1} | rise W {:3.0} R {:3.0} | cents {:+5.1} | level {:+5.1} | off atk {:+5.1}/{:+5.1} sus {:+5.1}/{:+5.1} rise {:+4.0}/{:+4.0} lvl {:+4.1}/{:+4.1} cents {:+4.1} | R stretch {:+5.1} off {:+4.1} | W heard-built {:+4.1}",
                name(key), m[1][0][i], m[0][0][i], m[1][1][i], m[0][1][i], m[1][2][i], m[0][2][i], cents[i], level[i],
                off[1][0][i], off[0][0][i], off[1][1][i], off[0][1][i], off[1][2][i], off[0][2][i], off[1][3][i], off[0][3][i], cents_off[i], stretch[i], stretch_off[i], built[i]);
        }
        println!("{label} spread (rms off trend) W / R: atk {:.1} / {:.1} dB, sus {:.1} / {:.1} dB, rise {:.1} / {:.1} ms, level {:.1} / {:.1} dB; W-R cents {:.1} rms off trend, mean {:+.1}",
            rms_of(&off[1][0]), rms_of(&off[0][0]), rms_of(&off[1][1]), rms_of(&off[0][1]), rms_of(&off[1][2]), rms_of(&off[0][2]), rms_of(&off[1][3]), rms_of(&off[0][3]),
            rms_of(&cents_off), cents.iter().filter(|c| c.is_finite()).sum::<f64>() / cents.iter().filter(|c| c.is_finite()).count().max(1) as f64);
        println!("{label} R stretch rms off trend {:.1} cents; W heard against built rms {:.1} cents", rms_of(&stretch_off), rms_of(&built));
        for (what, k, floor) in [("atk", 0usize, 3.0f64), ("sus", 1, 3.0), ("level", 3, 2.0)] {
            let spread = rms_of(&off[0][k]);
            let out: Vec<String> = (0..keys.len())
                .filter(|&i| off[1][k][i].abs() > floor.max(2.0 * spread) && !(off[0][k][i].abs() > 0.5 * off[1][k][i].abs() && off[0][k][i].signum() == off[1][k][i].signum()))
                .map(|i| format!("{} {:+.1}", name(keys[i]), off[1][k][i]))
                .collect();
            println!("{label} ours out of line, {what} (beyond {:.1}, the reference not with it): {out:?}", floor.max(2.0 * spread));
        }
    }
}

/// Where a treble key's brightness out of line with its neighbours comes
/// from: C4-C8 one at a time, the share above 4 kHz in the first 50 ms and
/// at 0.3-1.3 s (dB re the note) of the bridge velocity alone, then of the
/// dry output as built and with the duplex off or the board at 8 places, and
/// each one's distance from the median of the two keys either side; and the
/// felt (N at 1 mm) and its distance from its trend in %.
fn section_treblesource() {
    let keys: Vec<u8> = (60..=108).collect();
    let bridge_of = std::env::var("TREBLE_BRIDGE").ok();
    let names: Vec<&str> = if let Some(v) = bridge_of.as_deref() {
        vec![v]
    } else if std::env::args().any(|a| a == "bridge") {
        vec!["stock"]
    } else if std::env::args().any(|a| a == "strings") {
        vec!["stock", "unison-0", "even-0", "sideways-0", "phantoms-off", "direct-0", "knock-0", "coupling-0", "duplex-off"]
    } else {
        vec!["stock", "duplex-off", "board-8", "board-direct"]
    };
    let pianos: Vec<WaveguidePiano> = names.iter().map(|n| variant(n, true)).collect();
    let felt = pianos[0].felt_at_mm();
    let felt: Vec<f64> = keys.iter().map(|&k| felt[(k - 21) as usize]).collect();
    let felt_off = off_trend(&felt.iter().map(|f| f.ln()).collect::<Vec<_>>(), 2);
    if std::env::args().any(|a| a == "gains") {
        // What the board and the key's trim make of 6 kHz (the treble share's
        // band) and of partial 1, dB, and their distance from the trend.
        let p = &pianos[0];
        let at = |key: u8, hz: f64, regulated: bool| {
            let t = p.board_transfer(key, hz, regulated);
            db(0.5 * t.iter().map(|(re, im)| re * re + im * im).sum::<f64>())
        };
        let hf: Vec<f64> = keys.iter().map(|&k| at(k, 6000.0, true)).collect();
        let f1: Vec<f64> = keys.iter().map(|&k| at(k, p.scale()[(k - 21) as usize].f1, false)).collect();
        let (hf_off, f1_off) = (off_trend(&hf, 2), off_trend(&f1, 2));
        for (i, &key) in keys.iter().enumerate() {
            println!("{:>4} trimmed 6 kHz {:+6.1} off {:+5.1} | untrimmed partial 1 {:+6.1} off {:+5.1}", name(key), hf[i], hf_off[i], f1[i], f1_off[i]);
        }
        println!("spread: trimmed 6 kHz {:.1} dB, untrimmed partial 1 {:.1} dB", rms_of(&hf_off), rms_of(&f1_off));
        let mut wg = pianos[0].clone();
        for &key in &keys {
            println!("{}", design_row(&mut wg, key));
            // each partial of the loop against the string's, cents, to 8 kHz
            let k = wg.scale()[(key - 21) as usize].clone();
            let f = makepad_piano_waveguide::loop_filters(&k, fs());
            let t = tune(&f, k.f1, fs());
            let errs: String = (1..=16usize)
                .map(|n| (n, string_partial(k.f1, k.b, n)))
                .take_while(|p| p.1 < 12_000.0)
                .map(|(n, want)| format!(" {:+.1}", 1200.0 * (loop_partial(&f, t, n, fs()) / want).log2()))
                .collect();
            println!("     partials 1.. cents:{errs}");
        }
        return;
    }
    let len = 1.4;
    for vel in [60u8, 87] {
        // [source][atk/sus][key]: the bridge, then each variant's output
        let mut m = vec![vec![vec![0.0; keys.len()]; 2]; names.len() + 1];
        let mut force_share = vec![0.0; keys.len()];
        let mut contacts = vec![String::new(); keys.len()];
        for (i, &key) in keys.iter().enumerate() {
            let shares = |l: &[f32], r: &[f32]| {
                let o = onset(l, r);
                let (l, r) = (&l[o..], &r[o..]);
                [(0.0, 0.05), (0.3, 1.3)].map(|(a, z)| {
                    let s = spectrum(sect(l, a, z), sect(r, a, z));
                    db(band(&s, 4000.0, 20000.0) / band(&s, 20.0, 20000.0))
                })
            };
            let (v, force) = pianos[0].clone().blow_trace(key, vel, (len * fs()) as usize);
            let [a, z] = shares(&v, &v);
            (m[0][0][i], m[0][1][i]) = (a, z);
            let f = spectrum(&force[..(0.01 * fs()) as usize], &force[..(0.01 * fs()) as usize]);
            force_share[i] = db(band(&f, 4000.0, 20000.0) / band(&f, 20.0, 20000.0));
            // the hammer's contacts: (start, end) ms
            let mut on = None;
            let mut list = Vec::new();
            for (j, &x) in force.iter().enumerate().take((0.05 * fs()) as usize) {
                match (x > 0.0, on) {
                    (true, None) => on = Some(j),
                    (false, Some(a)) => {
                        list.push(format!("{:.2}-{:.2}", a as f64 * 1e3 / fs(), j as f64 * 1e3 / fs()));
                        on = None;
                    }
                    _ => {}
                }
            }
            contacts[i] = list.join(" ");
            for (j, p) in pianos.iter().enumerate() {
                let (l, r) = render(&mut p.clone(), &[(0.0, PianoEvent::NoteOn { key, velocity: vel })], len);
                let [a, z] = shares(&l, &r);
                (m[j + 1][0][i], m[j + 1][1][i]) = (a, z);
            }
        }
        let off: Vec<Vec<Vec<f64>>> = m.iter().map(|src| src.iter().map(|x| off_trend(x, 2)).collect()).collect();
        let force_off = off_trend(&force_share, 2);
        println!("== treblesource v{vel}: atk / sus share above 4 kHz off the trend of the 2 keys either side, for the hammer force (atk), the bridge, then {}; felt N and % off its trend", names.join(", "));
        for (i, &key) in keys.iter().enumerate() {
            let cols: String = (0..m.len()).map(|s| format!(" | {:+5.1}/{:+5.1}", off[s][0][i], off[s][1][i])).collect();
            println!("{:>4} v{vel} | {:+5.1}{cols} | felt {:6.1} N {:+4.0} % | contacts ms {}", name(key), force_off[i], felt[i], 100.0 * (felt_off[i].exp() - 1.0), contacts[i]);
        }
        let spreads: String = (0..m.len()).map(|s| format!(" {:.1}/{:.1}", rms_of(&off[s][0]), rms_of(&off[s][1]))).collect();
        println!("treblesource v{vel} spread atk/sus (bridge, {}):{spreads}", names.join(", "));
    }
}

/// A stop and a jump as Score sends them (AllSoundOff; at the jump the new
/// bar's note comes with it), in Score's callbacks of 512, through the old cut
/// (every state zeroed at once, as `reset` does, the pedal kept) and through
/// the instrument as it is. Writes out/stop-cut.wav and out/stop-fade.wav
/// under TRAUM_DIR and lists the clicks of each.
fn section_stop() {
    let out = std::path::PathBuf::from(std::env::var_os("TRAUM_DIR").expect("TRAUM_DIR")).join("out");
    let s = |t: f64| (t * fs()) as usize;
    let mut script: Vec<(usize, PianoEvent)> = vec![(0, PianoEvent::Sustain { value: 1.0 })];
    for at in [0.0, 1.5] {
        for key in [36u8, 52, 55, 60, 64, 67] {
            script.push((s(at), PianoEvent::NoteOn { key, velocity: 100 }));
        }
    }
    script.extend([(s(1.0), PianoEvent::AllSoundOff), (s(2.5), PianoEvent::AllSoundOff), (s(2.5), PianoEvent::NoteOn { key: 72, velocity: 80 })]);
    script.sort_by_key(|e| e.0);
    let total = s(3.5);
    println!("== stop at {:.0} Hz: a pedalled chord stopped at 1.0 s, again at 2.5 s with a new note sent with the stop", fs());
    for (label, cut) in [("stop-cut", true), ("stop-fade", false)] {
        let mut p = stock(true);
        let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
        let (mut pos, mut next) = (0, 0);
        while pos < total {
            let mut n = 512.min(total - pos);
            let mut events = Vec::new();
            while next < script.len() && script[next].0 < pos + n {
                let (at, event) = script[next];
                if cut && event == PianoEvent::AllSoundOff {
                    if at > pos {
                        n = at - pos;
                        break;
                    }
                    p.reset();
                    events.push(TimedEvent { offset: 0, event: PianoEvent::Sustain { value: 1.0 } });
                } else {
                    events.push(TimedEvent { offset: (at - pos) as u32, event });
                }
                next += 1;
            }
            p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
            pos += n;
        }
        let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        let clicks: Vec<String> = find_clicks(&mono, 20.0).iter().map(|c| format!("{:.3} s", c.0 as f64 / fs())).collect();
        println!("{label:<10} clicks {clicks:?}");
        write_wav(&out.join(format!("{label}.wav")), &l, &r);
    }
    // The same through the modal piano (the Concert Grand), dry as Score
    // builds it: into `stop-concert-grand.wav`.
    let preset = makepad_piano_model::PIANO_PRESETS.iter().find(|p| p.is_default).unwrap();
    let mut modal = makepad_piano_model::Piano::new_with_preset(fs() as f32, preset);
    modal.set_reverb_mix(0.0);
    modal.set_early_reflection_level(0.0);
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let (mut pos, mut next) = (0, 0);
    while pos < total {
        let n = 512.min(total - pos);
        let mut events = Vec::new();
        while next < script.len() && script[next].0 < pos + n {
            let (at, event) = script[next];
            let event = match event {
                PianoEvent::NoteOn { key, velocity } => makepad_piano_model::PianoEvent::NoteOn { key, velocity },
                PianoEvent::Sustain { value } => makepad_piano_model::PianoEvent::Sustain { value },
                _ => makepad_piano_model::PianoEvent::AllSoundOff,
            };
            events.push(makepad_piano_model::TimedEvent { offset: (at - pos) as u32, event });
            next += 1;
        }
        modal.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
    let clicks: Vec<String> = find_clicks(&mono, 20.0).iter().map(|c| format!("{:.3} s", c.0 as f64 / fs())).collect();
    println!("{:<10} clicks {clicks:?}", "stop-concert-grand");
    write_wav(&out.join("stop-concert-grand.wav"), &l, &r);
}

fn write_wav(path: &std::path::Path, l: &[f32], r: &[f32]) {
    let rate = fs() as u32;
    let frames = l.len().min(r.len());
    let data = (frames * 6) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 6).to_le_bytes());
    out.extend_from_slice(&6u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        for x in [l[i], r[i]] {
            let v = ((x as f64).clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
            out.extend_from_slice(&v.to_le_bytes()[..3]);
        }
    }
    std::fs::write(path, out).expect("write wav");
}

/// A0, C1, E1 and A1 at velocity 87 (what Score plays a written note at),
/// each held 2.3 s.
fn passage_bass() -> (Vec<(f64, Play)>, f64) {
    let mut script = Vec::new();
    for (i, k) in [21u8, 24, 28, 33].into_iter().enumerate() {
        let t = 0.05 + 2.6 * i as f64;
        script.push((t, Play::On(k, 87)));
        script.push((t + 2.3, Play::Off(k)));
    }
    (script, 10.9)
}

/// The pedalled 48-note passage with each key released after 0.4 s and
/// the pedal up at 7 s: many events, many open strings.
fn passage_dense() -> (Vec<(f64, Play)>, f64) {
    let mut script = vec![(0.0, Play::Pedal(1.0))];
    for i in 0..48u32 {
        let key = 24 + ((i * 29) % 72) as u8;
        script.push((0.125 * i as f64, Play::On(key, 60 + ((i * 13) % 50) as u8)));
        script.push((0.125 * i as f64 + 0.4, Play::Off(key)));
    }
    script.push((7.0, Play::Pedal(0.0)));
    (script, 8.5)
}

fn section_ab() {
    let Some(dir) = std::env::var_os("PROBE_OUT").or_else(|| std::env::var_os("DEVHUB_KEEP")) else {
        println!("== ab: set PROBE_OUT (or run under devhub run --keep) to say where the WAVs go");
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("output dir");
    let callback = arg("callback").unwrap_or(512.0) as usize;
    println!("== ab at {:.0} Hz, room {}, as Score plays (callbacks of {callback}), into {}: per passage the stock instrument at a K-weighted loudness of -20 dB and every switch matched to it", fs(), arg("room").map_or("as built".to_string(), |r| format!("{r}")), dir.display());
    for (name, about) in SWITCHES.iter().chain(MORE_SWITCHES.iter()) {
        println!("   {name}: {about}");
    }
    let colour: &[&str] = &["phantoms-off", "phantoms-resonant", "width-0", "ear-phase-0", "mono", "limiter-off", "board-direct", "duplex-off"];
    let bass: &[&str] = &["phantoms-off", "phantoms-resonant", "loss-high", "yield-1", "sideways-0", "bass-combined", "board-direct"];
    let crackle: &[&str] = &["aligned", "coupling-0", "limiter-off", "damper-slow", "phantoms-off"];
    let plan: [(&str, (Vec<(f64, Play)>, f64), &[&str]); 5] = [
        ("soft-note", passage_soft_note(), colour),
        ("forte-chord", passage_forte_chord(), colour),
        ("phrase", passage_phrase(), &["phantoms-off", "phantoms-resonant", "width-0", "ear-phase-0", "mono", "limiter-off", "board-direct", "duplex-off", "aligned", "coupling-0", "damper-slow", "bass-combined"]),
        ("bass", passage_bass(), bass),
        ("dense", passage_dense(), crackle),
    ];
    // `passages=bass+dense` renders only those; `stock` renders no switches
    let only: Option<Vec<String>> = std::env::args().find_map(|a| a.strip_prefix("passages=").map(|v| v.split('+').map(String::from).collect()));
    let stock_only = std::env::args().any(|a| a == "stock");
    for (label, (script, secs), switches) in plan {
        if only.as_ref().is_some_and(|o| !o.iter().any(|p| p == label)) {
            continue;
        }
        let switches: &[&str] = if stock_only { &[] } else { switches };
        let mut takes: Vec<(&str, Vec<f32>, Vec<f32>)> = Vec::new();
        for name in std::iter::once(&"stock").chain(switches.iter()) {
            let mut piano = variant(if *name == "mono" { "stock" } else { name }, false);
            let (mut l, mut r, _) = render_score(&mut piano, &script, secs, callback, (*name == "aligned").then_some(32));
            if *name == "mono" {
                for (a, b) in l.iter_mut().zip(r.iter_mut()) {
                    *a = 0.5 * (*a + *b);
                    *b = *a;
                }
            }
            takes.push((name, l, r));
        }
        let target = k_loudness(&takes[0].1, &takes[0].2);
        let mut gains: Vec<f64> = takes.iter().map(|(_, l, r)| (target / k_loudness(l, r)).sqrt()).collect();
        // stock to -20 dB, the rest alike; then the passage down as a whole if
        // anything would go over -1 dBFS
        let mut scale = (0.01 / target).sqrt();
        let peak = takes.iter().zip(&gains).map(|((_, l, r), g)| l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs())) as f64 * g * scale).fold(0.0, f64::max);
        if peak > 0.89 {
            scale *= 0.89 / peak;
        }
        for g in gains.iter_mut() {
            *g *= scale;
        }
        for ((name, l, r), g) in takes.iter().zip(&gains) {
            let l: Vec<f32> = l.iter().map(|x| x * *g as f32).collect();
            let r: Vec<f32> = r.iter().map(|x| x * *g as f32).collect();
            let file = format!("{label}-{name}.wav");
            write_wav(&dir.join(&file), &l, &r);
            let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, x| m.max(x.abs()));
            println!("{file:<34} gain {:+6.1} dB (re stock {:+5.1}) | peak {:+5.1} dBFS", db(g * g), db((g / gains[0]).powi(2)), db((peak * peak) as f64));
        }
    }
}

/// Decay in dB/s of a band around `f` over 0.1-1.5 s (0.2 s windows every
/// 0.1 s), and its level at 0.1-0.3 s.
fn band_decay(l: &[f32], r: &[f32], f: f64, half: f64) -> (f64, f64) {
    let times: Vec<f64> = (0..13).map(|i| 0.1 + 0.1 * i as f64).collect();
    let level: Vec<f64> = times.iter().map(|&t| db(band(&spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2)), f - half, f + half))).collect();
    (-slope(&times, &level), level[0])
}

/// `band_decay` over only the leading 0.1 s steps that stay within `range` dB
/// of the level at 0.1 s (at least three), so that a quiet late tail does
/// not set the slope.
fn band_decay_within(l: &[f32], r: &[f32], f: f64, half: f64, range: f64) -> (f64, f64) {
    let times: Vec<f64> = (0..13).map(|i| 0.1 + 0.1 * i as f64).collect();
    let level: Vec<f64> = times.iter().map(|&t| db(band(&spectrum(sect(l, t, t + 0.2), sect(r, t, t + 0.2)), f - half, f + half))).collect();
    let n = level.iter().position(|&v| v < level[0] - range).unwrap_or(level.len()).max(3);
    (-slope(&times[..n], &level[..n]), level[0])
}

fn section_bass() {
    let vel = arg("vel").unwrap_or(87.0) as u8;
    println!("== bass v{vel}, dry: per partial n = 1..40 (below 8 kHz), decay dB/s over 0.1-1.5 s from onset; each instrument at its own partial frequencies");
    let mut players: Vec<(&str, Player)> = vec![("stock", Player::Waveguide(stock(true)))];
    for name in ["phantoms-off", "loss-high", "yield-1", "sideways-0", "bass-combined"] {
        players.push((name, Player::Waveguide(variant(name, true))));
    }
    let [_, (_, modal)] = Player::both(true);
    players.push(("Concert Grand", modal));
    let fixture: Vec<(u8, u8, [f64; 11])> = FIXTURE.lines().filter(|l| !l.starts_with('#')).skip(1).map(|line| {
        let f: Vec<&str> = line.split('\t').collect();
        (f[0].parse().unwrap(), f[1].parse().unwrap(), std::array::from_fn(|i| f[10 + i].parse().unwrap()))
    }).collect();
    for key in [21u8, 24, 28, 30, 33, 36] {
        let mut b_line = format!("{:>3} B:", name(key));
        let mut band_line = String::new();
        for (pname, piano) in players.iter_mut() {
            let (f1, b) = piano.string(key);
            if matches!(*pname, "stock" | "Concert Grand") {
                b_line += &format!(" {pname} {b:.2e} (f1 {f1:.2})");
            }
            let (l, r) = piano.play(&[(0.0, Play::On(key, vel))], 1.8);
            let o = onset(&l, &r);
            let (l, r) = (&l[o..], &r[o..]);
            let mut decays = Vec::new();
            for n in 1..=40usize {
                let f = string_partial(f1, b, n);
                if f > 8000.0 {
                    break;
                }
                decays.push(band_decay(l, r, f, 0.4 * f1).0);
            }
            println!("{:>3} {pname:<14} {}", name(key), decays.iter().map(|d| format!("{d:5.1}")).collect::<Vec<_>>().join(""));
            let m = fixture_metrics(&l.to_vec(), &r.to_vec(), key);
            band_line += &format!(" | {pname} {:.1} {:.1} {:.1}", m[8], m[9], m[10]);
            let mean = |a: usize, z: usize| decays.get(a - 1..z.min(decays.len())).map_or(f64::NAN, |d| d.iter().sum::<f64>() / d.len().max(1) as f64);
            band_line += &format!(" (n10-20 {:.1}, 21-40 {:.1})", mean(10, 20), mean(21, 40));
        }
        println!("{b_line}");
        println!("{:>3} band decays dB/s 20-500 0.5-2k 2-8k (fixture definition){band_line}", name(key));
        for (fk, fv, m) in fixture.iter().filter(|r| r.0 == key) {
            println!("{:>3} recording v{fv}: {:.1} {:.1} {:.1}", name(*fk), m[8], m[9], m[10]);
        }
    }
}

fn main() {
    RATE.set(arg("rate").unwrap_or(48000.0)).unwrap();
    let args: Vec<String> = std::env::args().skip(1).filter(|a| !a.contains('=')).collect();
    let want = |s: &str| args.is_empty() || args.iter().any(|a| a == s);
    if want("design") {
        section_design();
    }
    if args.iter().any(|a| a == "scoreboard") {
        section_scoreboard();
        return;
    }
    if args.iter().any(|a| a == "scripts") {
        section_scripts();
        return;
    }
    if args.iter().any(|a| a == "spikes") {
        section_spikes();
        return;
    }
    if args.iter().any(|a| a == "contacts") {
        section_contacts();
        return;
    }
    if args.iter().any(|a| a == "hfsource") {
        section_hfsource();
        return;
    }
    if args.iter().any(|a| a == "traum") {
        section_traum();
        return;
    }
    if args.iter().any(|a| a == "treblesource") {
        section_treblesource();
        return;
    }
    if args.iter().any(|a| a == "bassdecay") {
        section_bassdecay();
        return;
    }
    if args.iter().any(|a| a == "m2") {
        section_m2();
        return;
    }
    if args.iter().any(|a| a == "limits") {
        section_limits();
        return;
    }
    if args.iter().any(|a| a == "felt") {
        section_felt();
        return;
    }
    if args.iter().any(|a| a == "engines") {
        section_engines();
        return;
    }
    if args.iter().any(|a| a == "regkeys") {
        section_regkeys();
        return;
    }
    if args.iter().any(|a| a == "touchab") {
        section_touchab();
    }
    if args.iter().any(|a| a == "topoctave") {
        section_topoctave();
        return;
    }
    if args.iter().any(|a| a == "touchkeys") {
        section_touchkeys();
    }
    if args.iter().any(|a| a == "boardkeys") {
        section_boardkeys();
        return;
    }
    if args.iter().any(|a| a == "touchfit") {
        section_touchfit();
        return;
    }
    if args.iter().any(|a| a == "chrom") {
        section_chrom();
        return;
    }
    if args.iter().any(|a| a == "decays") {
        section_decays();
        return;
    }
    if args.iter().any(|a| a == "stretch") {
        section_stretch();
        return;
    }
    if args.iter().any(|a| a == "treble") {
        section_treble();
        return;
    }
    if args.iter().any(|a| a == "stop") {
        section_stop();
        return;
    }
    if args.iter().any(|a| a == "timing") {
        section_timing();
        return;
    }
    if args.iter().any(|a| a == "reference") {
        section_reference();
        return;
    }
    if args.iter().any(|a| a == "colour" || a == "ab" || a == "clicks" || a == "bass") {
        if args.iter().any(|a| a == "clicks") {
            section_clicks();
        }
        if args.iter().any(|a| a == "bass") {
            section_bass();
        }
        if args.iter().any(|a| a == "colour") {
            section_colour();
        }
        if args.iter().any(|a| a == "ab") {
            section_ab();
        }
        return;
    }
    if want("ripple") {
        section_ripple();
    }
    if want("onset") {
        section_onset();
    }
    if want("release") {
        section_release();
    }
    if want("stereo") {
        section_stereo();
    }
    if want("halo") {
        section_halo();
    }
    if want("board") {
        section_board();
    }
    if want("cpu") {
        section_cpu();
    }
    if want("hammer") {
        section_hammer();
    }
    if want("ref") {
        section_ref();
    }
    if want("partials") {
        section_partials(&[21, 28, 36, 48, 60, 72, 84], 68);
    }
    if want("body") {
        section_body();
    }
    if want("scan") {
        section_scan(68, true);
        section_scan(28, false);
        section_scan(112, false);
    }
}
