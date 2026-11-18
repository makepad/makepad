use crate::design::{fft, loop_partial, string_partial, tune};
use crate::*;

const FS: f32 = 48_000.0;

/// The instrument is built once (that is the slow part) and copied.
fn stock() -> WaveguidePiano {
    static BUILT: std::sync::OnceLock<WaveguidePiano> = std::sync::OnceLock::new();
    BUILT.get_or_init(|| WaveguidePiano::new(FS)).clone()
}

fn dry() -> WaveguidePiano {
    let mut p = stock();
    p.set_room_mix(0.0);
    p
}

/// Render `secs` with events at sample positions, in blocks of 256: the
/// mean of the ears.
fn render(p: &mut WaveguidePiano, script: &[(usize, PianoEvent)], secs: f64) -> Vec<f32> {
    let (l, r) = render_ears(p, script, secs);
    l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect()
}

fn render_ears(p: &mut WaveguidePiano, script: &[(usize, PianoEvent)], secs: f64) -> (Vec<f32>, Vec<f32>) {
    let total = (secs * FS as f64) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut pos = 0;
    while pos < total {
        let n = 256.min(total - pos);
        let events: Vec<TimedEvent> =
            script.iter().filter(|(at, _)| *at >= pos && *at < pos + n).map(|&(at, event)| TimedEvent { offset: (at - pos) as u32, event }).collect();
        p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    assert!(l.iter().chain(r.iter()).all(|x| x.is_finite()), "output is finite");
    (l, r)
}

/// The strings and their dampers alone: nothing else left ringing.
fn bare() -> WaveguidePiano {
    let mut p = dry();
    p.set_duplex(0.0);
    p.set_coupling(0.0);
    p
}

/// (correlation of left and right, left over right level in dB) of one
/// mezzo-forte note's first half second.
fn image(key: u8) -> (f64, f64) {
    let mut p = dry();
    let (mut l, mut r) = (vec![0.0f32; at(0.5)], vec![0.0f32; at(0.5)]);
    p.process(&[TimedEvent { offset: 0, event: on(key, 70) }], &mut l, &mut r);
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(&x, &y)| x as f64 * y as f64).sum::<f64>();
    (dot(&l, &r) / (dot(&l, &l) * dot(&r, &r)).sqrt(), 10.0 * (dot(&l, &l) / dot(&r, &r)).log10())
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn at(secs: f64) -> usize {
    (secs * FS as f64) as usize
}

/// Energy of the sample-to-sample difference over the energy: rises with
/// the share of high frequencies.
fn brightness(x: &[f32]) -> f64 {
    let diff: f64 = x.windows(2).map(|w| ((w[1] - w[0]) as f64).powi(2)).sum();
    diff / x.iter().map(|&v| (v as f64).powi(2)).sum::<f64>()
}

fn on(key: u8, velocity: u8) -> PianoEvent {
    PianoEvent::NoteOn { key, velocity }
}

#[test]
fn every_loop_carries_the_stiff_strings_partials() {
    let p = dry();
    for midi in [21u8, 33, 45, 60, 72, 84, 96, 108] {
        let k = &p.scale()[(midi - 21) as usize];
        let filters = loop_filters(k, FS as f64);
        let tuning = tune(&filters, k.f1, FS as f64);
        for n in 1..=8usize {
            let want = string_partial(k.f1, k.b, n);
            if want > 10_000.0 {
                break;
            }
            let cents = 1200.0 * (loop_partial(&filters, tuning, n, FS as f64) / want).log2();
            let limit = if n == 1 { 0.01 } else { 6.0 };
            assert!(cents.abs() < limit, "key {midi} partial {n}: {cents:.2} cents from the stiff string");
        }
    }
    // The scale is stretched: octaves wider than 2:1, more so at the ends.
    let f = |midi: u8| p.scale()[(midi - 21) as usize].f1;
    assert!((f(69) - 440.0).abs() < 1e-6, "A4 is 440 Hz");
    assert!(f(108) > 440.0 * 2f64.powf(39.0 / 12.0) * 1.005, "the top is tuned sharp");
    // ...but not so far that it sounds sharp: about where tuners leave a
    // concert grand's treble (C7 near +11 cents, C8 near +40).
    let stretch = |midi: u8| 1200.0 * (f(midi) / (440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0))).log2();
    for (midi, low, high) in [(84, 1.5, 5.0), (96, 7.0, 15.0), (108, 30.0, 46.0)] {
        assert!((low..high).contains(&stretch(midi)), "key {midi} is tuned {:+.1} cents from equal temperament, not {low}..{high}", stretch(midi));
    }
    assert!(f(21) < 27.5 * 0.998, "the bottom is tuned flat");
    // In the lowest octave the octaves are tuned on 10:5: there the low
    // partials are faint, and the 6:3 octaves used before left the 10th
    // partial beating against the octave above's 5th, and A0-E1 3-6 cents
    // sharp of the reference AU's bass.
    let partial = |midi: u8, n: usize| {
        let k = &p.scale()[(midi - 21) as usize];
        string_partial(k.f1, k.b, n)
    };
    for midi in [21u8, 24, 26] {
        let cents = 1200.0 * (partial(midi, 10) / partial(midi + 12, 5)).log2();
        assert!(cents.abs() < 0.3, "key {midi}'s 10th partial is {cents:+.1} cents from the 5th of its octave");
    }
}

/// A unison's strings, tuned a fraction of a cent apart, stay that close
/// all the way up their partials: within a cent more than at partial 1, up
/// to 12 kHz, on every key.
/// Strings on either side of a whole sample of delay used to disagree by
/// 2-8 cents at 5 kHz and 7-49 at 10 kHz, on two keys in three from E4 up,
/// so their upper partials beat fast and those keys sounded edgy and sharp
/// beside clean neighbours.
#[test]
fn a_unisons_strings_agree_up_their_partials() {
    let p = stock();
    for (key, scale) in p.keys.iter().zip(p.scale()) {
        for n in 1..=64 {
            if string_partial(scale.f1, scale.b, n) > 12_000.0 {
                break;
            }
            let f = key.voice.string_partials(n);
            let (lo, hi) = f.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)));
            let cents = 1200.0 * (hi / lo).log2();
            let f1 = key.voice.string_partials(1);
            let tuned = 1200.0 * (f1.iter().cloned().fold(f64::MIN, f64::max) / f1.iter().cloned().fold(f64::MAX, f64::min)).log2();
            assert!(cents < tuned + 1.0, "key {} partial {n}: its strings {cents:.1} cents apart, {tuned:.1} at partial 1", scale.midi);
        }
    }
}

/// The treble speaks evenly: from C4 up, the treble in each key's first
/// 50 ms at v60 (its share of power above about 4 kHz, through a smooth
/// second-order edge, so that a partial crossing 4 kHz is no step) stays
/// within 3 dB of the median of the two keys either side. The reference
/// AU's keys scatter 0.7-0.9 dB rms about that trend, one key at 3.2 dB.
/// Share in dB of a note's first 50 ms above 4 kHz (a Butterworth
/// high-pass), both ears.
fn attack_share(p: &mut WaveguidePiano, key: u8, velocity: u8) -> f64 {
    let (k, q) = ((std::f64::consts::PI * 4000.0 / FS as f64).tan(), std::f64::consts::FRAC_1_SQRT_2);
    let a0 = 1.0 + k / q + k * k;
    let (b0, a1, a2) = (1.0 / a0, 2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0);
    p.reset();
    let (mut l, mut r) = (vec![0.0f32; at(0.05)], vec![0.0f32; at(0.05)]);
    p.process(&[TimedEvent { offset: 0, event: on(key, velocity) }], &mut l, &mut r);
    let (mut high, mut all) = (0.0, 0.0);
    for x in [&l, &r] {
        let mut s = [0.0f64; 4];
        for &v in x {
            let v = v as f64;
            let y = b0 * (v - 2.0 * s[0] + s[1]) - a1 * s[2] - a2 * s[3];
            s = [v, s[0], y, s[2]];
            high += y * y;
            all += v * v;
        }
    }
    10.0 * (high / all).log10()
}

#[test]
fn the_treble_speaks_evenly() {
    let mut p = dry();
    let keys: Vec<u8> = (58..=108).collect();
    let share: Vec<f64> = keys.iter().map(|&key| attack_share(&mut p, key, 60)).collect();
    let off: Vec<(u8, f64)> = (2..keys.len())
        .map(|i| {
            let mut near: Vec<f64> = share[i - 2..(i + 3).min(keys.len())].to_vec();
            near.sort_by(f64::total_cmp);
            (keys[i], share[i] - near[near.len() / 2])
        })
        .collect();
    let worst = off.iter().map(|o| o.1.abs()).fold(0.0, f64::max);
    let loud: Vec<String> = off.iter().filter(|o| o.1.abs() > 1.5).map(|o| format!("{} {:+.1}", o.0, o.1)).collect();
    assert!(worst < 3.0, "a treble key's attack is {worst:.1} dB off its neighbours' (beyond 1.5 dB: {loud:?})");
}

/// The strike point moves smoothly from key to key at any sample rate: from
/// E6 to C#7, partial 1 at the bridge (a mezzo-forte blow, its first 0.3 s)
/// steps by under 2.5 dB from one key to the next (the largest, 2 dB at
/// F#6-G6, is where the bridge's resistance steps) and comes out within
/// 0.6 dB alike at 44.1 and 48 kHz. With the span of the strike point's
/// comb rounded to whole samples it went from 2 to 3 samples between two
/// keys, other keys at each rate, and the whole key stepped there by 4.5-5
/// dB (A6-A#6 at 44.1 kHz, A#6-B6 at 48 kHz); E6 and A#6 came out 2.5 and
/// 5 dB apart between the two rates.
#[test]
fn the_strike_point_moves_smoothly_at_any_rate() {
    let voicing = voice::Voicing::default();
    let scale = scale::build_scale();
    let keys = 88..=97u8;
    let partial_1 = |fs: f64| -> Vec<f64> {
        keys.clone()
            .map(|key| {
                let k = &scale[(key - 21) as usize];
                let mut v = voice::Voice::new(k, fs, &voicing);
                v.set_damper(0.0);
                v.strike(voice::VOICING_SPEED);
                let mut x = vec![0.0f32; ((0.3 * fs) as usize).next_multiple_of(board::BLOCK)];
                for chunk in x.chunks_mut(board::BLOCK) {
                    v.process(None, chunk, &mut [0.0; board::BLOCK]);
                }
                let step = std::f64::consts::TAU * string_partial(k.f1, k.b, 1) / fs;
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &s) in x.iter().enumerate() {
                    let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / x.len() as f64).cos();
                    re += hann * s as f64 * (step * i as f64).cos();
                    im += hann * s as f64 * (step * i as f64).sin();
                }
                // the amplitude, whatever the number of samples
                20.0 * ((re * re + im * im).sqrt() / x.len() as f64).log10()
            })
            .collect()
    };
    let (cd, studio) = (partial_1(44_100.0), partial_1(48_000.0));
    let step = |x: &[f64]| x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f64::max);
    let apart = cd.iter().zip(&studio).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
    let shown: Vec<String> = keys.zip(cd.iter().zip(&studio)).map(|(key, (a, b))| format!("{key} {a:.1}/{b:.1}")).collect();
    assert!(step(&cd) < 2.5 && step(&studio) < 2.5 && apart < 0.6, "per key, partial 1 at the bridge in dB at 44.1 / 48 kHz: {shown:?}");
}

/// The top octave's tone does not depend on the sample rate: partial 2
/// against partial 1 at the bridge over a mezzo-forte blow's first 50 ms,
/// F7 to C8, alike within 1 dB at 44.1, 48 and 96 kHz. With every loop run
/// at the output's rate the shortest ones (A7-C8: 2-3 samples of delay at
/// 44.1 kHz, besides their filters) could not hold the strike's taps, the
/// strike span was held at one sample and the blow's length moved by a
/// tenth of a period, and from F7 up partial 2 came out 2.7-10.5 dB apart
/// between the rates (G7 -24.4 / -18.2 / -14.2 dB, B7 -12.1 / -3.0 / -1.6).
#[test]
fn the_top_octave_sounds_alike_at_any_rate() {
    let voicing = voice::Voicing::default();
    let scale = scale::build_scale();
    let keys = 101..=108u8;
    let second = |fs: f64| -> Vec<f64> {
        keys.clone()
            .map(|key| {
                let k = &scale[(key - 21) as usize];
                let mut v = voice::Voice::new(k, fs, &voicing);
                v.strike(voice::VOICING_SPEED);
                let mut x = vec![0.0f32; ((0.05 * fs) as usize).next_multiple_of(board::BLOCK)];
                for chunk in x.chunks_mut(board::BLOCK) {
                    v.process(None, chunk, &mut [0.0; board::BLOCK]);
                }
                x.truncate((0.05 * fs) as usize);
                let (bin, power) = spectrum(&x, &x);
                let s = (bin * fs / FS as f64, power);
                let part = |n: usize| {
                    let f = string_partial(k.f1, k.b, n);
                    band_db(&s, f - 0.4 * k.f1, f + 0.4 * k.f1)
                };
                part(2) - part(1)
            })
            .collect()
    };
    let rates = [44_100.0, 48_000.0, 96_000.0].map(second);
    let apart = (0..rates[0].len()).map(|i| rates.iter().map(|r| r[i]).fold(f64::MIN, f64::max) - rates.iter().map(|r| r[i]).fold(f64::MAX, f64::min)).fold(0.0, f64::max);
    let shown: Vec<String> = keys.enumerate().map(|(i, key)| format!("{key} {:.1}/{:.1}/{:.1}", rates[0][i], rates[1][i], rates[2][i])).collect();
    assert!(apart < 1.0, "per key, partial 2 against 1 at the bridge in dB at 44.1 / 48 / 96 kHz: {shown:?}");
}

/// Neighbouring keys of the top octave sound alike: partial 2 against
/// partial 1 at the bridge over a blow's first 50 ms steps by at most 3 dB
/// from one key to the next, F7 to C8, at v40, v60 and v110. Voiced to a
/// contact that grew from 1.15 periods at F7 to 1.74 at C8, partial 2 and
/// then partial 1 neared the nulls of the blow's spectrum (near-linear felt
/// gives a pulse close to a half-sine) and the steps reached 8.7, 14.0 and
/// 7.4 dB at v40, v60 and v110 (G7 to C8).
#[test]
fn the_top_octave_steps_evenly() {
    let mut p = dry();
    let scale = p.scale().to_vec();
    let keys = 101..=108u8;
    let mut shown = Vec::new();
    let mut worst = 0.0f64;
    for velocity in [40u8, 60, 110] {
        let second: Vec<f64> = keys
            .clone()
            .map(|key| {
                let k = &scale[(key - 21) as usize];
                let x = p.bridge_velocity(key, velocity, (0.05 * FS) as usize);
                let s = spectrum(&x, &x);
                let part = |n: usize| {
                    let f = string_partial(k.f1, k.b, n);
                    band_db(&s, f - 0.4 * k.f1, f + 0.4 * k.f1)
                };
                part(2) - part(1)
            })
            .collect();
        worst = second.windows(2).map(|w| (w[1] - w[0]).abs()).fold(worst, f64::max);
        shown.push(format!("v{velocity}: {:.1?}", second));
    }
    assert!(worst <= 3.0, "partial 2 against 1 at the bridge, F7-C8, dB; neighbours step by up to {worst:.1}: {shown:?}");
}

/// Above about C5 the strings' longitudinal modes leave the audible range
/// and the phantom partials fade out over a few keys, the same keys at any
/// sample rate. Cut off at a fifth of the rate they stopped between E5 and
/// F5 at 44.1 kHz and between F5 and F#5 at 48 kHz, each from full
/// strength to nothing.
#[test]
fn the_phantoms_fade_out_alike_at_any_rate() {
    let voicing = voice::Voicing::default();
    let scale = scale::build_scale();
    let keys = 71..=79u8;
    // per key, the phantoms' share of a hard blow's bridge motion over its
    // first 0.3 s, dB (-200: none)
    let phantoms = |fs: f64| -> Vec<f64> {
        keys.clone()
            .map(|key| {
                let k = &scale[(key - 21) as usize];
                let play = |strength: f64| {
                    let mut v = voice::Voice::new(k, fs, &voicing);
                    v.set_phantoms(strength);
                    v.set_damper(0.0);
                    v.strike(hammer_speed(110));
                    let mut x = vec![0.0f32; ((0.3 * fs) as usize).next_multiple_of(board::BLOCK)];
                    for chunk in x.chunks_mut(board::BLOCK) {
                        v.process(None, chunk, &mut [0.0; board::BLOCK]);
                    }
                    x
                };
                let (with, without) = (play(voicing.phantoms), play(0.0));
                let added: Vec<f32> = with.iter().zip(&without).map(|(a, b)| a - b).collect();
                (20.0 * (rms(&added) / rms(&without)).log10()).max(-200.0)
            })
            .collect()
    };
    let (cd, studio) = (phantoms(44_100.0), phantoms(48_000.0));
    let shown: Vec<String> = keys.clone().zip(cd.iter().zip(&studio)).map(|(key, (a, b))| format!("{key} {a:.1}/{b:.1}")).collect();
    let alike = cd.iter().zip(&studio).all(|(a, b)| (a - b).abs() < 1.0);
    // the last key that has any has them 20 dB under B4's: no step from
    // full strength to none (where nothing fades a key's level differs by
    // up to half a dB between the rates)
    let faded = |x: &[f64]| x[0] > -100.0 && x.iter().rev().find(|&&l| l > -200.0).is_some_and(|&last| last < x[0] - 20.0);
    assert!(alike && faded(&cd) && faded(&studio), "per key, the phantoms against the motion in dB at 44.1 / 48 kHz: {shown:?}");
}

/// Mean of the two ears' one-sided power spectra (Hann, padded to a power
/// of two): the bin width in Hz and the power per bin.
fn spectrum(l: &[f32], r: &[f32]) -> (f64, Vec<f64>) {
    let n = l.len().next_power_of_two();
    let w: Vec<f64> = (0..l.len()).map(|i| 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / l.len() as f64).cos()).collect();
    let mut power = vec![0.0; n / 2 + 1];
    for ch in [l, r] {
        let (mut re, mut im) = (vec![0.0; n], vec![0.0; n]);
        for (i, &v) in ch.iter().enumerate() {
            re[i] = v as f64 * w[i];
        }
        fft(&mut re, &mut im);
        for (k, p) in power.iter_mut().enumerate() {
            *p += 0.5 * (re[k] * re[k] + im[k] * im[k]);
        }
    }
    (FS as f64 / n as f64, power)
}

/// Power of a spectrum from `lo` to `hi` Hz, in dB.
fn band_db(s: &(f64, Vec<f64>), lo: f64, hi: f64) -> f64 {
    let (a, b) = ((lo / s.0).ceil() as usize, ((hi / s.0).ceil() as usize).min(s.1.len()));
    10.0 * s.1[a..b].iter().sum::<f64>().max(1e-30).log10()
}

/// The board is regulated as a technician regulates it: each key is evened
/// to the board's response over the third of an octave around it, and
/// further only where a mode of the board on its fundamental makes it stand
/// out by more than 3 dB. Over C4-C7 at v60, each key against the median of
/// the five around it: the level of the first 0.5 s spreads by under
/// 1.9 dB and steps by 2.9 dB on average and 7.5 at most; partial 1 within
/// 2.1, 2.9 and 7.7 dB; what the board makes of the 4-8 kHz band of the
/// first 50 ms steps by under 1.3 dB on average; and no more than one key in
/// six is evened beyond the smoothed level. Evened on its exact fundamental,
/// every key was lifted or cut whole to hide the board's dip or peak at
/// that one frequency, and its 4-8 kHz band then stepped by 2.4 dB on
/// average and by up to 7 dB from its neighbour's.
#[test]
fn the_board_is_regulated_over_its_register() {
    let mut p = dry();
    let keys: Vec<u8> = (58..=98).collect();
    let (mut level, mut fundamental, mut high) = (Vec::new(), Vec::new(), Vec::new());
    for &key in &keys {
        let k = p.scale()[(key - 21) as usize].clone();
        let v = p.bridge_velocity(key, 60, at(0.05));
        let (l, r) = render_ears(&mut p, &[(0, on(key, 60))], 0.5);
        p.reset();
        let energy: f64 = l.iter().zip(&r).map(|(&a, &b)| 0.5 * ((a as f64).powi(2) + (b as f64).powi(2))).sum();
        level.push(10.0 * (energy / l.len() as f64).log10());
        let f = string_partial(k.f1, k.b, 1);
        fundamental.push(band_db(&spectrum(&l, &r), f - 0.4 * k.f1, f + 0.4 * k.f1));
        high.push(band_db(&spectrum(&l[..at(0.05)], &r[..at(0.05)]), 4000.0, 8000.0) - band_db(&spectrum(&v, &v), 4000.0, 8000.0));
    }
    let inside: Vec<usize> = (0..keys.len()).filter(|&i| (60..=96).contains(&keys[i])).collect();
    // (spread against the median of five, mean step, largest step) over C4-C7
    let stats = |x: &[f64]| -> [f64; 3] {
        let dev = |i: usize| {
            let mut near = x[i - 2..i + 3].to_vec();
            near.sort_by(f64::total_cmp);
            x[i] - near[2]
        };
        let steps: Vec<f64> = inside.windows(2).map(|w| (x[w[1]] - x[w[0]]).abs()).collect();
        [(inside.iter().map(|&i| dev(i).powi(2)).sum::<f64>() / inside.len() as f64).sqrt(), steps.iter().sum::<f64>() / steps.len() as f64, steps.iter().copied().fold(0.0, f64::max)]
    };
    let (level, fundamental, high) = (stats(&level), stats(&fundamental), stats(&high));
    let evened: Vec<u8> = inside.iter().map(|&i| keys[i]).filter(|&key| p.regulation(key)[1] != 0.0).collect();
    let within = |x: [f64; 3], bound: [f64; 3]| x.iter().zip(bound).all(|(x, b)| *x <= b);
    assert!(
        within(level, [1.9, 2.9, 7.5]) && within(fundamental, [2.1, 2.9, 7.7]) && high[1] <= 1.3 && evened.len() <= inside.len() / 6,
        "C4-C7 (spread, mean step, largest step in dB): level {level:.1?}, partial 1 {fundamental:.1?}, 4-8 kHz {high:.1?}; evened beyond the smoothed level: {evened:?}"
    );
}

/// The step from single strings to pairs (E1 to F1) is not heard: a pair
/// takes twice the load off the hammer, and voiced to the singles' contact
/// its felt came out a quarter as hard and F1-A#1 8 dB duller above 4 kHz
/// than the singles below and C2 above, where the reference AU's attack
/// runs on evenly (within 2 dB).
#[test]
fn the_break_to_pairs_of_strings_is_even() {
    let mut p = dry();
    let mut mean = |keys: std::ops::RangeInclusive<u8>| {
        let n = keys.clone().count() as f64;
        keys.map(|key| attack_share(&mut p, key, 60)).sum::<f64>() / n
    };
    let (singles, pairs) = (mean(26..=28), mean(29..=34));
    assert!((pairs - singles).abs() < 3.0, "attack above 4 kHz, v60: F1-A#1 {pairs:.1} dB, D1-E1 {singles:.1} dB");
}

/// Mean decay in dB/s from 0.1 to 0.9 s of each band of partials of a v60
/// note from the dry instrument.
fn partial_decays(key: u8, bands: &[std::ops::RangeInclusive<usize>]) -> Vec<f64> {
    let mut p = dry();
    let x = render(&mut p, &[(0, on(key, 60))], 1.1);
    let k = &p.scale()[(key - 21) as usize];
    // power of partial `n` in the 0.2 s from `t`, Hann-windowed
    let power = |n: usize, t: f64| -> f64 {
        let w = &x[at(t)..at(t + 0.2)];
        let step = std::f64::consts::TAU * string_partial(k.f1, k.b, n) / FS as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, &v) in w.iter().enumerate() {
            let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / w.len() as f64).cos();
            let (s, c) = (step * i as f64).sin_cos();
            re += hann * v as f64 * c;
            im += hann * v as f64 * s;
        }
        re * re + im * im
    };
    bands
        .iter()
        .map(|ns| ns.clone().map(|n| 10.0 * (power(n, 0.1) / power(n, 0.9)).log10() / 0.8).sum::<f64>() / ns.clone().count() as f64)
        .collect()
}

/// The lowest octave rings long in its lowest partials and drains its
/// upper ones sooner, as a grand's does: below the board's lowest modes
/// the bridge hardly yields. From 0.1 to 0.9 s of a v60 note, partials 1-3
/// of A0, C#1 and E1 lose under 4 dB/s and partials 25-40 at least 1.5 dB/s
/// more. They lost 6-8 dB/s and the upper ones 3-5, so the bottom died thin
/// and kept a wiry ring (the reference AU: about 2 and 8).
#[test]
fn the_lowest_octave_rings_long_below_and_drains_its_top() {
    let found: Vec<(u8, Vec<f64>)> = [21u8, 25, 28].into_iter().map(|key| (key, partial_decays(key, &[1..=3, 25..=40]))).collect();
    let shown: Vec<String> = found.iter().map(|(key, d)| format!("{key}: {:.1} / {:.1}", d[0], d[1])).collect();
    assert!(found.iter().all(|(_, d)| d[0] < 4.0 && d[1] > d[0] + 1.5), "dB/s of partials 1-3 / 25-40: {shown:?}");
}

/// The tenor's lowest partials sing on: from 0.1 to 0.9 s of a v60 note,
/// partials 1-3 of A2, D3 and G3 lose under 13 dB/s, about the reference
/// AU's 7-15. They lost up to 21, so the tenor sounded short beside it.
#[test]
fn the_tenor_sings_on() {
    let found: Vec<(u8, f64)> = [45u8, 50, 55].into_iter().map(|key| (key, partial_decays(key, &[1..=3])[0])).collect();
    let shown: Vec<String> = found.iter().map(|(key, d)| format!("{key}: {d:.1}")).collect();
    assert!(found.iter().all(|&(_, d)| d < 13.0), "dB/s of partials 1-3: {shown:?}");
}

/// The top octave rings as long as the bridge it is sized for lets it:
/// the strings' own loss at partial 1 is part of the note's prompt decay,
/// not added to it. With even unisons, so that only the strings and the
/// bridge set the decay, partial 1 of C7, F#7 and C8 loses under 45 dB/s
/// from 0.1 to 0.5 s (the reference AU's: 25-40). It lost 40, 60 and 112:
/// the top notes died twice as fast as they should.
/// Partial `n`'s level in dB, Hann-windowed over 0.1 s, every 0.1 s from
/// 0.1 s to `until`: the stock instrument, dry, one key at v60.
fn partial_levels(key: u8, n: usize, until: f64) -> Vec<f64> {
    let mut p = dry();
    let x = render(&mut p, &[(0, on(key, 60))], until + 0.1);
    let k = &p.scale()[(key - 21) as usize];
    let step = std::f64::consts::TAU * string_partial(k.f1, k.b, n) / FS as f64;
    (1..=(until * 10.0).round() as usize)
        .map(|i| {
            let w = &x[at(0.1 * i as f64)..at(0.1 * (i + 1) as f64)];
            let (mut re, mut im) = (0.0, 0.0);
            for (j, &v) in w.iter().enumerate() {
                let hann = 0.5 - 0.5 * (std::f64::consts::TAU * j as f64 / w.len() as f64).cos();
                let (s, c) = (step * j as f64).sin_cos();
                re += hann * v as f64 * c;
                im += hann * v as f64 * s;
            }
            10.0 * (re * re + im * im).max(1e-30).log10()
        })
        .collect()
}

/// The treble rings as the reference AU's does. Around middle C the
/// fundamental sings on (C4, D4 and E4's partial 1 lose under 15 dB/s); in
/// the top octave partial 1 falls steadily for 1.5 s: once fallen, it never
/// comes back by more than 5 dB, where a detuned unison against the
/// bridge's fast drain would leave a null and a plateau 15 dB above it.
#[test]
fn the_treble_rings_on_then_fades_steadily() {
    let middle: Vec<f64> = [60u8, 62, 64].into_iter().map(|key| partial_decays(key, &[1..=1])[0]).collect();
    assert!(middle.iter().sum::<f64>() / 3.0 < 15.0, "C4, D4, E4 partial 1 dB/s: {middle:.1?}");
    for key in [100u8, 103, 106] {
        let levels = partial_levels(key, 1, 1.5);
        let mut low = f64::INFINITY;
        let back = levels.iter().fold(0.0f64, |back, &l| {
            low = low.min(l);
            back.max(l - low)
        });
        assert!(back < 5.0, "key {key}: partial 1 comes back {back:.1} dB after falling: {levels:.0?}");
    }
}

/// The top octave decays as its bridge is sized: with clean unisons and an
/// even strike, partial 1 loses no more than the prompt decay its bridge is
/// sized for (within 15 %).
#[test]
fn the_top_octave_decays_as_its_bridge_is_sized() {
    let voicing = voice::Voicing { unison_cents: 0.0, strike_unevenness: 0.0, ..voice::Voicing::default() };
    let scale = scale::build_scale();
    let found: Vec<(u8, f64)> = [96u8, 102, 108]
        .into_iter()
        .map(|key| {
            let k = &scale[(key - 21) as usize];
            let mut v = voice::Voice::new(k, FS as f64, &voicing);
            v.set_damper(0.0);
            v.strike(voice::VOICING_SPEED);
            let mut x = vec![0.0f32; at(0.6)];
            for chunk in x.chunks_mut(board::BLOCK) {
                v.process(None, chunk, &mut [0.0; board::BLOCK]);
            }
            // power of partial 1 in the 0.1 s from `t`, Hann-windowed
            let power = |t: f64| -> f64 {
                let w = &x[at(t)..at(t + 0.1)];
                let step = std::f64::consts::TAU * string_partial(k.f1, k.b, 1) / FS as f64;
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &s) in w.iter().enumerate() {
                    let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / w.len() as f64).cos();
                    re += hann * s as f64 * (step * i as f64).cos();
                    im += hann * s as f64 * (step * i as f64).sin();
                }
                re * re + im * im
            };
            (key, 10.0 * (power(0.1) / power(0.5)).log10() / 0.4)
        })
        .collect();
    let shown: Vec<String> = found.iter().map(|(key, d)| format!("{key}: {d:.1} of {:.1}", scale::prompt_decay_db(*key as f64))).collect();
    assert!(found.iter().all(|&(key, d)| d < 1.15 * scale::prompt_decay_db(key as f64)), "dB/s of partial 1 against the bridge's: {shown:?}");
}

/// A treble note strikes and clears: partial 1 of a held v60 note falls by
/// at least 14 dB from 50-100 ms to 450-500 ms at G5, C6, A6 and E7 (the
/// reference AU: 19 dB, the median of G5-C7, and 26 over the top octave).
/// With the bridge sized to the mean of a fast first stage and a slow ring
/// it fell 9.0, 7.3 and 6.8 dB at G5, C6 and A6 (15.5 at E7), and a note
/// hung on under its neighbours.
#[test]
fn the_treble_strikes_and_clears() {
    let mut p = dry();
    let scale = p.scale().to_vec();
    let found: Vec<(u8, f64)> = [79u8, 84, 93, 100]
        .into_iter()
        .map(|key| {
            let k = &scale[(key - 21) as usize];
            p.reset();
            let x = render(&mut p, &[(0, PianoEvent::NoteOn { key, velocity: 60 })], 0.55);
            // power of partial 1 in the 50 ms from `t`, Hann-windowed
            let power = |t: f64| -> f64 {
                let w = &x[at(t)..at(t + 0.05)];
                let step = std::f64::consts::TAU * string_partial(k.f1, k.b, 1) / FS as f64;
                let (mut re, mut im) = (0.0, 0.0);
                for (i, &s) in w.iter().enumerate() {
                    let hann = 0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / w.len() as f64).cos();
                    re += hann * s as f64 * (step * i as f64).cos();
                    im += hann * s as f64 * (step * i as f64).sin();
                }
                re * re + im * im
            };
            (key, 10.0 * (power(0.05) / power(0.45)).log10())
        })
        .collect();
    assert!(found.iter().all(|&(_, d)| d >= 14.0), "partial 1, 50-100 ms against 450-500 ms, dB: {found:.1?}");
}

/// The treble keeps its level up to C6, as the reference AU's does: the
/// first 0.3 s of F#5-G#5 and of B5-C#6 at v60 (each the mean of the three
/// keys) are within 2 dB of middle C's. They fell 4 and 5 dB below it.
#[test]
fn the_treble_keeps_its_level_up_to_c6() {
    let mut p = dry();
    let mut level = |key: u8| -> f64 {
        p.reset();
        let x = render(&mut p, &[(0, on(key, 60))], 0.3);
        20.0 * rms(&x).log10()
    };
    let middle_c = level(60);
    let mean = |keys: [u8; 3], level: &mut dyn FnMut(u8) -> f64| keys.iter().map(|&k| level(k)).sum::<f64>() / 3.0 - middle_c;
    let (g5, c6) = (mean([78, 79, 80], &mut level), mean([83, 84, 85], &mut level));
    assert!(g5.abs() < 2.0 && c6.abs() < 2.0, "dB re middle C: F#5-G#5 {g5:+.1}, B5-C#6 {c6:+.1}");
}

/// Ordinary loud playing passes the limiter untouched: v80 chords in the
/// middle and the treble and a v110 chord in the middle peak under its
/// knee. A v110 chord in the treble crosses it on a few peaks only (under
/// 0.2 % of the samples of its first 0.4 s), and nothing reaches full scale.
#[test]
fn loud_playing_stays_under_the_limiter() {
    let chord = |keys: &[u8], velocity: u8| -> Vec<(usize, PianoEvent)> { keys.iter().map(|&k| (100, on(k, velocity))).collect() };
    let (middle, treble) = ([48u8, 52, 55, 60, 64, 67, 72], [72u8, 76, 79, 84, 88, 91, 96]);
    let mut free = stock();
    free.set_limit_knee(1.0);
    let mut peaks = |keys: &[u8], velocity: u8| -> Vec<f32> {
        free.reset();
        let (l, r) = render_ears(&mut free, &chord(keys, velocity), 0.4);
        l.into_iter().chain(r).map(f32::abs).collect()
    };
    let dbfs = |x: &[f32]| 20.0 * x.iter().fold(0.0f32, |m, &v| m.max(v)).log10();
    for (keys, velocity) in [(&middle[..], 80), (&treble[..], 80), (&middle[..], 110)] {
        let level = dbfs(&peaks(keys, velocity));
        assert!(level < 20.0 * KNEE.log10(), "a v{velocity} chord from key {} peaks at {level:+.1} dBFS, in the limiter", keys[0]);
    }
    let loud = peaks(&treble, 110);
    let over = loud.iter().filter(|&&v| v > KNEE).count() as f64 / loud.len() as f64;
    assert!(over < 2e-3, "a v110 treble chord has {:.3} % of its samples over the limiter's knee", 100.0 * over);
    let (l, r) = render_ears(&mut stock(), &chord(&treble, 110), 0.4);
    let out: Vec<f32> = l.into_iter().chain(r).map(f32::abs).collect();
    assert!(dbfs(&out) < 0.0, "a v110 treble chord peaks at {:+.1} dBFS out of the limiter", dbfs(&out));
}

#[test]
fn a_note_speaks_at_its_offset_then_only_decays() {
    let mut p = dry();
    let x = render(&mut p, &[(100, on(60, 90))], 1.3);
    assert!(x[..100].iter().all(|&v| v == 0.0), "silent before the event's offset");
    assert!(rms(&x[100..at(0.05)]) > 1e-4, "sounds right after it");
    let windows: Vec<f64> = (1..6).map(|i| rms(&x[at(0.2 * i as f64)..at(0.2 * (i + 1) as f64)])).collect();
    for w in windows.windows(2) {
        assert!(w[1] < w[0], "the bridge only takes energy out: {windows:?}");
    }
    // Middle C loses 8 to 40 dB in a second while it is held (the
    // reference AU's about 10).
    let drop = 20.0 * (windows[0] / windows[4]).log10() / 0.8;
    assert!((8.0..40.0).contains(&drop), "C4 decays at {drop:.1} dB/s");
}

#[test]
fn tone_follows_touch() {
    let (mut soft, mut hard) = (dry(), dry());
    let a = render(&mut soft, &[(0, on(60, 30))], 0.4);
    let b = render(&mut hard, &[(0, on(60, 110))], 0.4);
    assert!(rms(&b) > 2.5 * rms(&a), "a harder blow is louder: {} vs {}", rms(&b), rms(&a));
    assert!(brightness(&b) > 2.0 * brightness(&a), "and brighter: {} vs {}", brightness(&b), brightness(&a));
    let (ta, fa) = soft.last_blow(60);
    let (tb, fb) = hard.last_blow(60);
    assert!(tb < 0.85 * ta && fb > 3.0 * fa, "the felt leaves sooner and pushes harder: {ta} s {fa} N, {tb} s {fb} N");
    // Voiced contact time at mezzo forte, as the scale asks.
    let mut mf = dry();
    render(&mut mf, &[(0, on(60, 67))], 0.05);
    let want = mf.scale()[39].contact * Voicing::default().contact;
    let (got, _) = mf.last_blow(60);
    assert!((got / want - 1.0).abs() < 0.1, "C4 contact {got} s, voiced for {want} s");
}

/// A harder touch is as much louder in every register, as the reference
/// AU's notes are: as the touch law says, 18 dB from v40 to v110 and 32
/// from v20 to v127, within 1.5 dB from the bass to the top. Before the
/// touch regulation the treble's felt made E4-A5 grow 4.6-5.2 dB more
/// than the law from v40 to v110 and 8.3-9.0 dB more from v20 to v127.
/// The regulation's spans step by under half a dB from key to key. The
/// brightening that comes with a harder touch stays in the bass and the
/// middle, little of it at the top.
#[test]
fn every_register_grows_alike_with_touch() {
    let mut p = dry();
    let mut level = |key: u8, velocity: u8| {
        p.reset();
        20.0 * rms(&render(&mut p, &[(0, on(key, velocity))], 0.5)).log10()
    };
    let law = |soft: u8, hard: u8| 20.0 * (hammer_speed(hard) / hammer_speed(soft)).log10();
    let off: Vec<(u8, f64, f64)> =
        [21u8, 33, 45, 57, 64, 69, 76, 81, 88, 96].iter().map(|&key| (key, level(key, 110) - level(key, 40) - law(40, 110), level(key, 127) - level(key, 20) - law(20, 127))).collect();
    assert!(off.iter().all(|&(_, mid, whole)| mid.abs() < 1.5 && whole.abs() < 1.5), "per key, growth v40 to v110 and v20 to v127 over the touch law's, dB: {off:.1?}");
    let p = dry();
    let step = (FIRST_KEY..LAST_KEY).map(|k| (20.0 * (p.touch(k + 1) / p.touch(k)).log10()).abs()).fold(0.0f32, f32::max);
    assert!(step < 0.5, "the touch spans step by {step:.2} dB between neighbours");
    let mut p = dry();
    let mut brighter = |key: u8| attack_share(&mut p, key, 110) - attack_share(&mut p, key, 40);
    let (low, top) = (brighter(33), brighter(96));
    assert!(low > 10.0 && top < 6.0, "attack above 4 kHz, v40 to v110: A1 {low:+.1} dB, C7 {top:+.1} dB");
}

#[test]
fn dampers_and_pedals_do_their_work() {
    let held = |script: &[(usize, PianoEvent)]| {
        let x = render(&mut bare(), script, 0.9);
        rms(&x[at(0.7)..])
    };
    let off = (at(0.2), PianoEvent::NoteOff { key: 60 });
    let ringing = held(&[(0, on(60, 90))]);
    let released = held(&[(0, on(60, 90)), off]);
    let pedalled = held(&[(0, PianoEvent::Sustain { value: 1.0 }), (0, on(60, 90)), off]);
    let half = held(&[(0, PianoEvent::Sustain { value: 0.45 }), (0, on(60, 90)), off]);
    let sostenuto = held(&[(0, on(60, 90)), (at(0.1), PianoEvent::Sostenuto { on: true }), off]);
    let late_sostenuto = held(&[(0, on(60, 90)), off, (at(0.21), PianoEvent::Sostenuto { on: true })]);
    assert!(released < 0.02 * ringing, "release damps: {released} vs {ringing}");
    // (Not quite the held level: the strings the pedal frees take up some
    // of the bridge's motion before they give it back.)
    assert!(pedalled > 0.7 * ringing, "the pedal holds the note: {pedalled} vs {ringing}");
    assert!(half > 2.0 * released && half < 0.5 * pedalled, "half pedal is in between: {half}");
    assert!(sostenuto > 0.9 * ringing, "sostenuto holds a key that was down: {sostenuto}");
    assert!(late_sostenuto < 0.02 * ringing, "but not one already released: {late_sostenuto}");
    // The top strings have no dampers.
    let top = |script: &[(usize, PianoEvent)]| {
        let x = render(&mut bare(), script, 0.5);
        rms(&x[at(0.3)..])
    };
    let rung = top(&[(0, on(100, 90))]);
    let let_go = top(&[(0, on(100, 90)), (at(0.1), PianoEvent::NoteOff { key: 100 })]);
    assert!(let_go > 0.9 * rung, "E7 rings on after release: {let_go} vs {rung}");
}

#[test]
fn una_corda_is_softer_and_duller() {
    let plain = render(&mut dry(), &[(0, on(64, 80))], 0.4);
    let soft = render(&mut dry(), &[(0, PianoEvent::SoftPedal { on: true }), (0, on(64, 80))], 0.4);
    assert!(rms(&soft) < 0.9 * rms(&plain), "softer: {} vs {}", rms(&soft), rms(&plain));
    assert!(brightness(&soft) < 0.9 * brightness(&plain), "duller: {} vs {}", brightness(&soft), brightness(&plain));
}

#[test]
fn all_sound_off_fades_out() {
    // AllSoundOff (Score's stop, or a jump to another bar) takes the sound
    // away on a short slope: a cut from full level is a click. A note sent
    // with it still plays.
    let mut p = dry();
    let mut script: Vec<(usize, PianoEvent)> = [36u8, 48, 55, 60, 64, 79].iter().map(|&k| (0, on(k, 110))).collect();
    script.push((at(0.2), PianoEvent::AllSoundOff));
    script.push((at(0.2), on(72, 80)));
    let x = render(&mut p, &script, 0.5);
    let cut: Vec<f64> = spikes(&x[at(0.1)..at(0.3)]).iter().map(|&i| i as f64 / FS as f64 + 0.1).collect();
    assert!(cut.is_empty(), "clicks at {cut:.4?} s");
    assert!(rms(&x[at(0.3)..]) > 0.02 * rms(&x[at(0.1)..at(0.2)]), "the note sent with it plays");
}

#[test]
fn a_released_piano_comes_to_rest() {
    let mut p = stock();
    let mut script = vec![(0, PianoEvent::Sustain { value: 1.0 })];
    for (i, key) in [36u8, 60, 67, 84, 21, 108].into_iter().enumerate() {
        script.push((i * 500, on(key, 100)));
        script.push((at(0.2), PianoEvent::NoteOff { key }));
    }
    script.push((at(0.25), PianoEvent::Sustain { value: 0.0 }));
    // out-of-range notes are ignored
    script.push((0, on(5, 100)));
    script.push((0, on(120, 100)));
    // The damperless top (C8) is stopped by AllSoundOff.
    script.push((at(0.3), PianoEvent::AllSoundOff));
    let x = render(&mut p, &script, 0.6);
    assert!(rms(&x[..at(0.2)]) > 1e-3, "it played");
    assert!(x[at(0.31)..].iter().all(|&v| v == 0.0), "exact silence after AllSoundOff");
    // Without it the damper stops the note at once; the damperless top
    // strings and the duplex segments it set ringing fade by themselves,
    // to exact silence.
    let mut p = stock();
    let x = render(&mut p, &[(0, on(72, 100)), (at(0.1), PianoEvent::NoteOff { key: 72 })], 12.0);
    let played = rms(&x[..at(0.1)]);
    assert!(rms(&x[at(0.5)..at(1.0)]) < 0.01 * played, "the note is damped: {} vs {played}", rms(&x[at(0.5)..at(1.0)]));
    assert!(x[at(11.5)..].iter().all(|&v| v == 0.0), "strings, segments, board and room all at rest: {}", rms(&x[at(11.5)..]));
}

#[test]
fn undamped_strings_ring_along() {
    // A C4 held for a moment leaves the damperless top strings ringing
    // where its partials meet theirs: C7 is its eighth partial.
    let script = [(0, on(60, 110)), (at(0.6), PianoEvent::NoteOff { key: 60 })];
    let mut p = dry();
    p.set_duplex(0.0);
    let x = render(&mut p, &script, 1.4);
    let halo = rms(&x[at(1.1)..]);
    let mut alone = bare();
    let y = render(&mut alone, &script, 1.4);
    assert!(halo > 5.0 * rms(&y[at(1.1)..]), "the top rings on after the damper: {halo} vs {}", rms(&y[at(1.1)..]));
    assert!(p.active_count >= 1 && alone.active_count == 0, "and only strings without dampers do: {} awake", p.active_count);
    assert!(halo < 0.02 * rms(&x[..at(0.1)]), "far below the note itself: {halo}");
    // With the pedal down the whole instrument answers a single note, and
    // nothing in that exchange gains energy.
    let mut p = dry();
    let z = render(&mut p, &[(0, PianoEvent::Sustain { value: 1.0 }), (0, on(48, 100))], 2.0);
    let windows: Vec<f64> = (1..5).map(|i| rms(&z[at(0.4 * i as f64)..at(0.4 * (i + 1) as f64)])).collect();
    for w in windows.windows(2) {
        assert!(w[1] < w[0], "pedalled C3 only decays: {windows:?}");
    }
    assert!(p.active_count > 40, "most of the instrument is ringing: {} keys", p.active_count);
}

/// Spikes in `x` after its first millisecond: samples whose second
/// difference stands 20 dB over its RMS in the 5 ms around them (the 0.25 ms
/// around each left out), at most one per 2 ms.
fn spikes(x: &[f32]) -> Vec<usize> {
    let y: Vec<f64> = (0..x.len()).map(|i| if i < 2 { 0.0 } else { x[i] as f64 - 2.0 * x[i - 1] as f64 + x[i - 2] as f64 }).collect();
    let mut acc = vec![0.0f64; y.len() + 1];
    for (i, v) in y.iter().enumerate() {
        acc[i + 1] = acc[i] + v * v;
    }
    let (wide, near) = (at(0.005), at(0.00025));
    let mut found: Vec<usize> = Vec::new();
    for i in at(0.001).max(wide)..y.len().saturating_sub(wide) {
        let around = ((acc[i + wide] - acc[i - wide]) - (acc[i + near + 1] - acc[i - near])) / (2 * (wide - near) - 1) as f64;
        if y[i] * y[i] > 100.0 * around && y[i] * y[i] > 1e-20 && found.last().map_or(true, |&f| i - f >= at(0.002)) {
            found.push(i);
        }
    }
    found
}

#[test]
fn every_hammer_meets_its_strings_through_felt() {
    // Felt so hard that it stops dead on the string throws a sharp edge into
    // it, which runs round the loop as a buzz of clicks; a hammer on felt
    // gives the bridge a smooth blow, soft or loud.
    let mut p = dry();
    let mut buzzing = Vec::new();
    for key in FIRST_KEY..=LAST_KEY {
        for velocity in [40u8, 87] {
            let found = spikes(&p.bridge_velocity(key, velocity, at(0.12)));
            if !found.is_empty() {
                buzzing.push(format!("{key} v{velocity}: {} spikes from {:.1} ms (felt on for {:.2} ms)", found.len(), found[0] as f64 * 1e3 / FS as f64, p.last_blow(key).0 * 1e3));
            }
        }
    }
    assert!(buzzing.is_empty(), "keys whose blow buzzes: {buzzing:?}");
}

#[test]
fn no_hammer_is_stone_among_felt() {
    // The voicing chases each key's contact time; where that time is out of
    // reach of felt like the neighbours', it must not end on felt like
    // stone: that puts a wiry ring into the strings that stays in soft
    // playing (it made D2-G2 sound like a sitar).
    let p = dry();
    let felt = p.felt_at_mm();
    let mut hard = Vec::new();
    for (i, &f) in felt.iter().enumerate() {
        let trend = crate::felt_trend(&felt, i);
        if f > 2.0 * trend * (1.0 + 1e-9) {
            hard.push(format!("{}: {f:.0} N at 1 mm against {trend:.0} N around it", i + FIRST_KEY as usize));
        }
    }
    assert!(hard.is_empty(), "felt much harder than its neighbours': {hard:?}");
    // And what that does to a soft note: its strings' share above 4 kHz.
    let share = |key: u8| {
        let v = p.clone().bridge_velocity(key, 60, at(0.6));
        let x = &v[at(0.05)..];
        // energy of a steep high-pass (fourth difference) over the energy
        let hp: f64 = x.windows(5).map(|w| ((w[4] - 4.0 * w[3] + 6.0 * w[2] - 4.0 * w[1] + w[0]) as f64).powi(2)).sum();
        10.0 * (hp / x.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()).log10()
    };
    let (a1, c3) = (share(33), share(48));
    let shares: Vec<(u8, f64)> = (36..=45u8).map(|key| (key, share(key))).collect();
    let worst = shares.iter().map(|s| s.1).fold(f64::MIN, f64::max);
    assert!(worst < a1.max(c3) + 12.0, "C2-A2 high shares {shares:.1?} dB against A1 {a1:.1} and C3 {c3:.1}");
}

#[test]
fn a_blow_knocks_on_the_case() {
    // Under a high note the knock is what there is below the note itself.
    let low = |x: &[f32]| {
        // two one-pole low-passes at about 400 Hz, then RMS
        let (mut y, mut z, mut sum) = (0.0f64, 0.0f64, 0.0f64);
        for &v in x {
            y += 0.05 * (v as f64 - y);
            z += 0.05 * (y - z);
            sum += z * z;
        }
        (sum / x.len() as f64).sqrt()
    };
    let play = |knock: f64, velocity: u8| {
        let mut p = dry();
        p.set_knock(knock);
        low(&render(&mut p, &[(0, on(96, velocity))], 0.12))
    };
    let (with, without, soft) = (play(BoardShape::default().knock, 100), play(0.0, 100), play(BoardShape::default().knock, 40));
    assert!(with > 3.0 * without, "the case answers the blow: {with} vs {without}");
    assert!(soft < 0.6 * with, "less for a softer blow: {soft} vs {with}");
}

#[test]
fn phantom_partials_come_with_force() {
    // The strings' longitudinal components are quadratic in the motion:
    // an edge on a hard bass note, nothing on a soft one.
    let play = |phantoms: f64, velocity: u8| {
        let mut p = dry();
        p.set_phantoms(phantoms);
        p.set_knock(0.0);
        brightness(&render(&mut p, &[(0, on(33, velocity))], 0.4)[at(0.05)..])
    };
    let strength = Voicing::default().phantoms;
    let (hard, hard_without) = (play(strength, 120), play(0.0, 120));
    let (soft, soft_without) = (play(strength, 25), play(0.0, 25));
    let (edge_hard, edge_soft) = (hard / hard_without, soft / soft_without);
    assert!(edge_hard > 1.15, "a hard A1 gains an edge: {edge_hard}");
    assert!(edge_hard > 1.5 * edge_soft.max(1.0) - 0.5 && edge_soft < edge_hard, "more than a soft one: {edge_soft} vs {edge_hard}");
}

#[test]
fn duplex_segments_ring_after_the_note() {
    // A damped C5 leaves the segments behind its bridge ringing at their
    // own pitches; they are what is left a moment after the release.
    let script = [(0, on(72, 100)), (at(0.15), PianoEvent::NoteOff { key: 72 })];
    let tail = |duplex: f64| {
        let mut p = dry();
        p.set_duplex(duplex);
        p.set_coupling(0.0);
        let x = render(&mut p, &script, 0.9);
        (rms(&x[at(0.5)..at(0.7)]), rms(&x[at(0.7)..]), rms(&x[..at(0.15)]))
    };
    let (early, late, note) = tail(BoardShape::default().duplex);
    let (without, _, _) = tail(0.0);
    assert!(early > 5.0 * without, "the segments ring on: {early} vs {without}");
    assert!(late < early, "and fade: {late} after {early}");
    assert!(early < 0.03 * note, "well under the note: {early} vs {note}");
}

#[test]
fn two_ears_hear_the_bass_together_and_the_middle_apart() {
    // Two listening points over the board: long waves reach both alike,
    // shorter ones with their own arrival at each.
    let mean = |keys: &[u8]| keys.iter().map(|&k| image(k).0).sum::<f64>() / keys.len() as f64;
    let (bass, middle) = (mean(&[24, 28, 33, 36]), mean(&[55, 57, 59, 61, 63, 65]));
    assert!(bass > 0.6, "the bass is one image: {bass}");
    assert!(middle < bass - 0.05 && middle > 0.0, "the middle is wider, not opposed: {middle} vs {bass}");
    // The top speaks through the direct path, which differs in level only.
    let (top, balance) = image(100);
    assert!(top > 0.9 && balance.abs() < 4.0, "E7 sits near the centre: {top}, {balance} dB");
}

#[test]
fn a_damper_takes_the_top_first_and_longer_in_the_bass() {
    // What is left 100-200 ms after the release, against just before it.
    let left = |key: u8| {
        let x = render(&mut bare(), &[(0, on(key, 80)), (at(0.3), PianoEvent::NoteOff { key })], 0.5);
        (rms(&x[at(0.4)..]) / rms(&x[at(0.25)..at(0.3)]), brightness(&x[at(0.32)..at(0.4)]) / brightness(&x[at(0.2)..at(0.3)]))
    };
    let ((bass, bass_tone), (middle, middle_tone)) = (left(33), left(60));
    assert!(bass > 0.1 && bass < 0.6, "a bass string pushes through the felt for a while: {bass}");
    assert!(middle < 0.3 * bass, "a middle one is stopped sooner: {middle} vs {bass}");
    assert!(bass_tone < 0.9 && middle_tone < 0.9, "the upper partials go first: {bass_tone}, {middle_tone}");
}
