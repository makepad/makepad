//! Tandem against renders of the reference (TESTS.md C): the suite's
//! patches played with its events, measured the way the renders were
//! (peak, RMS, zero-crossing pitch over the loudest second, when the sound
//! starts and ends, the 10 ms RMS envelope), and compared. The renders and
//! the patches are data outside the repo: `TANDEM_REFRENDERS` names the
//! render folder (its `suite.txt` and `out/*.json`), `TANDEM_PATCH_LIBRARY`
//! the unpacked data folder; without both the test skips. Prints a table;
//! fails only past the starting thresholds of TESTS.md C3 loosened for
//! untuned modules (level 3 dB, pitch 1 % on steady single tones, end
//! 0.15 s).

use makepad_audio_synth::tandem::{LoadMode, Patch, Tandem};
use std::path::PathBuf;

const RATE: f32 = 48_000.0;

#[derive(Debug)]
enum Ev {
    On(u8, u8),
    Off(u8),
    Bend(i32),
}

fn events(text: &str) -> Vec<(f32, Ev)> {
    let mut out = Vec::new();
    for e in text.split(',') {
        let Some((kind, rest)) = e.trim().split_once('@') else { continue };
        let parts: Vec<&str> = rest.split(':').collect();
        let t: f32 = parts[0].parse().unwrap_or(0.0);
        let num = |i: usize| parts.get(i).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0);
        match kind {
            "on" => out.push((t, Ev::On(num(1) as u8, num(2) as u8))),
            "off" => out.push((t, Ev::Off(num(1) as u8))),
            "bend" => out.push((t, Ev::Bend(num(1)))),
            _ => {}
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

struct Stats {
    peak_db: f64,
    rms_db: f64,
    first: Option<f64>,
    last: Option<f64>,
    zc_hz: f64,
    env: Vec<f64>,
}

fn db(x: f64) -> f64 {
    if x > 0.0 { 20.0 * x.log10() } else { -999.0 }
}

/// As the renders' own measure (per channel).
fn stats(x: &[f32]) -> Stats {
    let sr = RATE as usize;
    let n = x.len();
    let peak = x.iter().fold(0.0f64, |m, v| m.max(v.abs() as f64));
    let rms = (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
    let w = sr / 100;
    let env = (0..n).step_by(w).map(|i| {
        let s = &x[i..(i + w).min(n)];
        db((s.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt())
    }).collect();
    let thr = 10f64.powf(-60.0 / 20.0);
    let first = x.iter().position(|v| (v.abs() as f64) > thr).map(|i| i as f64 / sr as f64);
    let last = x.iter().rposition(|v| (v.abs() as f64) > thr).map(|i| i as f64 / sr as f64);
    let (mut best, mut bi) = (-1.0f64, 0usize);
    let mut i = 0;
    while i < (n.saturating_sub(sr)).max(1) {
        let e: f64 = x[i..(i + sr).min(n)].iter().step_by(8).map(|v| (*v as f64).powi(2)).sum();
        if e > best {
            best = e;
            bi = i;
        }
        i += w;
    }
    let seg = &x[bi..(bi + sr).min(n)];
    let zc = seg.windows(2).filter(|p| p[0] <= 0.0 && 0.0 < p[1]).count();
    Stats { peak_db: db(peak), rms_db: db(rms), first, last, zc_hz: zc as f64 * sr as f64 / seg.len().max(1) as f64, env }
}

fn json_num(text: &str, key: &str) -> Option<f64> {
    let at = text.find(&format!("\"{key}\""))?;
    let rest = &text[at + key.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with("null") {
        return None;
    }
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == 'E' || c == '+')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn json_env(text: &str) -> Vec<f64> {
    let Some(at) = text.find("\"rms_10ms_db\"") else { return Vec::new() };
    let rest = &text[at..];
    let (Some(a), Some(b)) = (rest.find('['), rest.find(']')) else { return Vec::new() };
    rest[a + 1..b].split(',').filter_map(|v| v.trim().parse().ok()).collect()
}

#[test]
fn suite_against_reference_renders() {
    let (Some(refs), Some(lib)) = (std::env::var_os("TANDEM_REFRENDERS").map(PathBuf::from), std::env::var_os("TANDEM_PATCH_LIBRARY").map(PathBuf::from)) else {
        eprintln!("tandem_refrenders: TANDEM_REFRENDERS / TANDEM_PATCH_LIBRARY not set: skipped");
        return;
    };
    let mut suite = std::fs::read_to_string(refs.join("suite.txt")).expect("suite.txt");
    // Stage-isolation variants: `./variants/<name>.fxp` is a library patch with the edits of
    // variants.txt (`name | source | element=value,...`), applied here.
    let mut isos: Vec<PathBuf> = std::fs::read_dir(&refs).map(|d| d.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
    isos.retain(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("suite-iso") && n.ends_with(".txt")));
    isos.sort();
    for iso in isos.iter().filter_map(|p| std::fs::read_to_string(p).ok()) {
        suite.push('\n');
        suite.push_str(&iso);
    }
    let variants: Vec<(String, String, String)> = std::fs::read_to_string(refs.join("variants.txt"))
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && l.contains('|'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('|').map(str::trim).collect();
            (f.len() >= 3).then(|| (f[0].to_string(), f[1].to_string(), f[2].to_string()))
        })
        .collect();
    let mut failures = Vec::new();
    eprintln!("{:<22} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} {:>8} {:>8} {:>7}", "render", "rms", "ref", "peak", "ref", "zc_hz", "ref", "end", "ref", "env_l1", "rms_r", "ref_r", "l1_r");
    for line in suite.lines().filter(|l| !l.trim_start().starts_with('#') && l.contains('|')) {
        let f: Vec<&str> = line.split('|').map(str::trim).collect();
        if f.len() < 4 {
            continue;
        }
        let (name, rel, secs, evs) = (f[0], f[1], f[2].parse::<f32>().unwrap_or(4.0), events(f[3]));
        let Ok(json) = std::fs::read_to_string(refs.join("out").join(format!("{name}.json"))) else {
            eprintln!("{name}: no reference render");
            continue;
        };
        let variant = rel.strip_prefix("./variants/").and_then(|v| v.strip_suffix(".fxp")).and_then(|v| variants.iter().find(|(n, ..)| n == v));
        let source = variant.map(|(_, src, _)| src.as_str()).unwrap_or(rel);
        let Ok(bytes) = std::fs::read(lib.join(source)) else {
            eprintln!("{name}: patch {source} not in the library");
            continue;
        };
        let mut patch = Patch::from_fxp(&bytes, name, LoadMode::Patch).expect("loads");
        if let Some((_, _, edits)) = variant {
            for e in edits.split(',') {
                if let Some((k, v)) = e.split_once('=') {
                    if let (Some(slot), Ok(v)) = (patch.value_mut(k.trim()), v.trim().parse::<f32>()) {
                        slot.v = v;
                    }
                }
            }
        }
        // The renders loaded the reference's installed copy of tutorial 07, whose script spells
        // `enumerator = 5` (a typo: that clock stays 1/4); the pinned library spells it right.
        if name.starts_with("formula-07") || name.starts_with("f07-") {
            for f in patch.formulae.iter_mut().flatten().flatten() {
                *f = f.replace("numerator = 5", "enumerator = 5");
            }
        }
        let mut t = Tandem::new(RATE);
        t.load(&patch);
        t.set_tempo(120.0);
        // The renders run the host for a 1 s preroll before the first event and drop it (effect
        // LFOs, smoothers and free-running modulators are mid-flight at t = 0): so do we.
        let (mut pl, mut pr) = ([0.0f32; 32], [0.0f32; 32]);
        for _ in 0..(RATE as usize / 32) {
            t.render(&mut pl, &mut pr);
        }
        let n = (secs * RATE) as usize;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        let mut at = 0usize;
        let mut next = 0usize;
        while at < n {
            while next < evs.len() && (evs[next].0 * RATE) as usize <= at {
                match evs[next].1 {
                    Ev::On(k, v) => t.note_on(0, k, v),
                    Ev::Off(k) => t.note_off(0, k, 64),
                    Ev::Bend(b) => t.pitch_bend(0, b),
                }
                next += 1;
            }
            let end = (at + 32).min(n);
            t.render(&mut l[at..end], &mut r[at..end]);
            at = end;
        }
        // Debug aid: TANDEM_REF_DUMP=<dir> writes each render as interleaved little-endian f32.
        if let Some(dir) = std::env::var_os("TANDEM_REF_DUMP") {
            let bytes: Vec<u8> = l.iter().zip(&r).flat_map(|(a, b)| [a.to_le_bytes(), b.to_le_bytes()]).flatten().collect();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(PathBuf::from(dir).join(format!("{name}.f32")), bytes);
        }
        let s = stats(&l);
        // The reference's left channel.
        let ch0 = &json[json.find("\"channels\"").unwrap_or(0)..];
        let (rrms, rpeak, rzc, rlast) = (json_num(ch0, "rms_dbfs").unwrap_or(-999.0), json_num(ch0, "peak_dbfs").unwrap_or(-999.0), json_num(ch0, "zero_cross_hz_loudest_1s").unwrap_or(0.0), json_num(ch0, "last_above_-60dB_s"));
        // The right channel too (stereo effects, Wide): ours and the reference's second channel.
        let sr_ = stats(&r);
        // Channel 1 starts after channel 0's envelope (its last field).
        let ch1 = ch0.find("\"rms_10ms_db\"").map(|i| &ch0[i + 13..]).unwrap_or("");
        let rrms_r = json_num(ch1, "rms_dbfs").unwrap_or(-999.0);
        let renv_r = json_env(ch1);
        let kr = renv_r.len().min(sr_.env.len());
        let l1_r = (0..kr).filter(|i| renv_r[*i] > -80.0 || sr_.env[*i] > -80.0).map(|i| (renv_r[i].max(-80.0) - sr_.env[i].max(-80.0)).abs()).sum::<f64>() / kr.max(1) as f64;
        let renv = json_env(ch0);
        let k = renv.len().min(s.env.len());
        let l1 = (0..k).filter(|i| renv[*i] > -80.0 || s.env[*i] > -80.0).map(|i| (renv[i].max(-80.0) - s.env[i].max(-80.0)).abs()).sum::<f64>() / k.max(1) as f64;
        eprintln!(
            "{:<22} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.1} {:>8.1} {:>7.3} {:>7.3} {:>7.2} {:>8.2} {:>8.2} {:>7.2}",
            name, s.rms_db, rrms, s.peak_db, rpeak, s.zc_hz, rzc, s.last.unwrap_or(-1.0), rlast.unwrap_or(-1.0), l1, sr_.rms_db, rrms_r, l1_r
        );
        let _ = s.first;
        let isolation = variant.is_some();
        if !isolation && (s.rms_db - rrms).abs() > 3.0 {
            failures.push(format!("{name}: level {:.2} dB vs {rrms:.2}", s.rms_db));
        }
        // Zero crossings measure pitch only for a single steady tone (not chords, bends or plucked noise).
        let steady = name.starts_with("init-") && name.ends_with("-c4") && !name.contains("karplus");
        if steady && rzc > 20.0 && (s.zc_hz / rzc - 1.0).abs() > 0.01 {
            failures.push(format!("{name}: pitch {:.1} Hz vs {rzc:.1}", s.zc_hz));
        }
        if let (Some(a), Some(b)) = (s.last, rlast) {
            if (a - b).abs() > 0.15 {
                failures.push(format!("{name}: ends at {a:.3} s vs {b:.3}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
