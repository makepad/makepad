//! Low-register verification renderer (offline, no audio device).
//!
//! Single notes: every note/velocity pair on a fresh instrument, NoteOn at
//! sample zero, optional NoteOff (`--release S`) and sustain pedal held down
//! from the start (`--pedal`). Files `note_021_vel_068.wav` (stereo float32)
//! plus `keys.tsv` with each key's design f0 and B for the analysis.
//! Passage: `--passage FILE.wav` renders a chromatic descent C3..A0 and a few
//! low chords into one file; `--events EVENTS.txt --passage FILE.wav` renders
//! an event list instead (lines `seconds on key velocity`, `seconds off key`,
//! `seconds pedal value`, e.g. converted from a MIDI performance).
//!
//! The instrument is `Piano::new` (stock calibration) unless `--raw`;
//! `--design name=value,...` overrides DesignParams in either mode (stock
//! keeps the stock calibration table), `--voicing name=value,...` overrides
//! Voicing. Output is dry (no reverb, reflections or limiter) unless `--wet`.
use makepad_piano_model::{
    calibration_data::DEFAULT_CALIBRATION, DesignParams, Piano, PianoEvent, TimedEvent, Voicing,
};
use std::{env, error::Error, fs, io::Write, path::PathBuf};

fn wav(path: &PathBuf, rate: u32, l: &[f32], r: &[f32]) -> Result<(), Box<dyn Error>> {
    let size = u32::try_from(l.len() * 8)?;
    let mut out = std::io::BufWriter::new(fs::File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(size + 36).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&3u16.to_le_bytes())?;
    out.write_all(&2u16.to_le_bytes())?;
    out.write_all(&rate.to_le_bytes())?;
    out.write_all(&(rate * 8).to_le_bytes())?;
    out.write_all(&8u16.to_le_bytes())?;
    out.write_all(&32u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&size.to_le_bytes())?;
    for (&a, &b) in l.iter().zip(r) {
        if !a.is_finite() || !b.is_finite() {
            return Err("non-finite model output".into());
        }
        out.write_all(&a.to_le_bytes())?;
        out.write_all(&b.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}

fn list(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::new();
    for part in text.split(',') {
        if let Some((a, b)) = part.split_once('-') {
            let (a, b): (u8, u8) = (a.parse()?, b.parse()?);
            out.extend(a..=b);
        } else {
            out.push(part.parse()?);
        }
    }
    Ok(out)
}

struct Setup {
    rate: f32,
    raw: bool,
    design: DesignParams,
    voicing: Voicing,
    wet: bool,
}

impl Setup {
    fn piano(&self) -> Piano {
        let mut p = if self.raw {
            Piano::new_with_params(self.rate, &self.design)
        } else {
            Piano::new_with_params_calibrated(self.rate, &self.design, DEFAULT_CALIBRATION)
        };
        p.set_voicing(self.voicing);
        if !self.wet {
            p.set_reverb_mix(0.0);
            p.set_early_reflection_level(0.0);
            p.set_soft_clip(false);
        }
        p
    }
}

/// Renders an absolute-time event script (seconds) in 256-sample blocks.
fn render(p: &mut Piano, rate: f32, script: &[(f64, PianoEvent)], seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let total = (seconds * rate as f64) as usize;
    let mut l = vec![0.0f32; total];
    let mut r = vec![0.0f32; total];
    let mut events = Vec::new();
    let mut pos = 0;
    while pos < total {
        let n = 256.min(total - pos);
        events.clear();
        for &(at, ev) in script {
            let s = (at * rate as f64).round() as usize;
            if s >= pos && s < pos + n {
                events.push(TimedEvent { offset: (s - pos) as u32, event: ev });
            }
        }
        events.sort_by_key(|e| e.offset);
        p.process(&events, &mut l[pos..pos + n], &mut r[pos..pos + n]);
        pos += n;
    }
    (l, r)
}

fn passage() -> (Vec<(f64, PianoEvent)>, f64) {
    let mut s = Vec::new();
    let on = |key, velocity| PianoEvent::NoteOn { key, velocity };
    let off = |key| PianoEvent::NoteOff { key };
    // chromatic descent C3..A0, 0.6 s per note, legato-ish
    let mut t = 0.2;
    for key in (21..=48u8).rev() {
        s.push((t, on(key, 72)));
        s.push((t + 0.55, off(key)));
        t += 0.6;
    }
    t += 1.2;
    // held single notes: A0, C1, A1, C2 (4 s each, forte)
    for key in [21u8, 24, 33, 36] {
        s.push((t, on(key, 96)));
        s.push((t + 3.6, off(key)));
        t += 4.2;
    }
    // chords (left hand low register), each held 3 s
    let chords: [&[u8]; 5] = [
        &[24, 36],         // C1 + C2 octave
        &[26, 33, 38],     // D1 A1 D2
        &[28, 35, 40, 44], // E1 B1 E2 G#2
        &[33, 40, 45, 48], // A1 E2 A2 C3
        &[21, 33, 40],     // A0 A1 E2
    ];
    for (i, chord) in chords.iter().enumerate() {
        let velocity = [80u8, 70, 90, 60, 110][i];
        for &k in chord.iter() {
            s.push((t, on(k, velocity)));
            s.push((t + 2.8, off(k)));
        }
        t += 3.3;
    }
    // pedalled arpeggio C1 G1 C2 E2 G2, pedal down throughout
    s.push((t - 0.05, PianoEvent::Sustain { value: 1.0 }));
    for (i, k) in [24u8, 31, 36, 40, 43].into_iter().enumerate() {
        s.push((t + 0.25 * i as f64, on(k, 75)));
        s.push((t + 0.25 * i as f64 + 0.2, off(k)));
    }
    t += 5.0;
    s.push((t, PianoEvent::Sustain { value: 0.0 }));
    (s, t + 2.0)
}

/// Parses an event list (see the header); the render runs 3 s past the last event.
fn events(text: &str) -> Result<(Vec<(f64, PianoEvent)>, f64), Box<dyn Error>> {
    let mut s = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let t: f64 = f[0].parse()?;
        let ev = match (f[1], f.len()) {
            ("on", 4) => PianoEvent::NoteOn { key: f[2].parse()?, velocity: f[3].parse()? },
            ("off", 3) => PianoEvent::NoteOff { key: f[2].parse()? },
            ("pedal", 3) => PianoEvent::Sustain { value: f[2].parse()? },
            _ => return Err(format!("bad event line: {line}").into()),
        };
        s.push((t, ev));
    }
    let end = s.iter().map(|e| e.0).fold(0.0, f64::max);
    Ok((s, end + 3.0))
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let mut out = None;
    let mut passage_out = None;
    let mut events_in: Option<PathBuf> = None;
    let mut notes = list("21-48")?;
    let mut velocities = vec![28u8, 68, 112];
    let mut seconds = 4.0;
    let mut release: Option<f64> = None;
    let mut pedal = false;
    let mut setup = Setup { rate: 48000.0, raw: false, design: DesignParams::default(), voicing: Voicing::default(), wet: false };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value()?)),
            "--passage" => passage_out = Some(PathBuf::from(value()?)),
            "--events" => events_in = Some(PathBuf::from(value()?)),
            "--notes" => notes = list(&value()?)?,
            "--velocities" => velocities = list(&value()?)?,
            "--seconds" => seconds = value()?.parse()?,
            "--release" => release = Some(value()?.parse()?),
            "--pedal" => pedal = true,
            "--raw" => setup.raw = true,
            "--wet" => setup.wet = true,
            "--design" => {
                for kv in value()?.split(',') {
                    let (k, v) = kv.split_once('=').ok_or("design needs name=value")?;
                    if !setup.design.set(k, v.parse()?) {
                        return Err(format!("unknown design parameter {k}").into());
                    }
                }
            }
            "--voicing" => {
                for kv in value()?.split(',') {
                    let (k, v) = kv.split_once('=').ok_or("voicing needs name=value")?;
                    let v: f32 = v.parse()?;
                    let vc = &mut setup.voicing;
                    match k {
                        "body_tap" => vc.body_tap = v,
                        "knock" => vc.knock = v,
                        "roughness" => vc.roughness = v,
                        "phantoms" => vc.phantoms = v,
                        "attack_noise" => vc.attack_noise = v,
                        "attack_body" => vc.attack_body = v,
                        "sympathetic" => vc.sympathetic = v,
                        _ => return Err(format!("unknown voicing field {k}").into()),
                    }
                }
            }
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let rate = setup.rate;
    if let Some(path) = passage_out {
        let (script, secs) = match &events_in {
            Some(path) => events(&fs::read_to_string(path)?)?,
            None => passage(),
        };
        let mut p = setup.piano();
        let (l, r) = render(&mut p, rate, &script, secs);
        let peak = l.iter().chain(&r).fold(0.0f32, |m, &x| m.max(x.abs()));
        // listening copy: dry renders are normalised to -1 dBFS peak
        let g = if setup.wet { 1.0 } else { 0.89 / peak.max(1e-9) };
        let (l, r): (Vec<f32>, Vec<f32>) = (l.iter().map(|x| x * g).collect(), r.iter().map(|x| x * g).collect());
        wav(&path, rate as u32, &l, &r)?;
        println!("passage {} s, peak {:.3} (gain {:.3})", secs, peak, g);
        return Ok(());
    }
    let out = out.ok_or("--out or --passage required")?;
    fs::create_dir_all(&out)?;
    let probe = setup.piano();
    let mut keys = String::from("key\tf0\tb\tstrings\tpartials\n");
    for &k in &notes {
        let info = probe.key_info(k).ok_or("key outside 21..108")?;
        keys += &format!("{k}\t{}\t{}\t{}\t{}\n", info.f0, info.b_coeff, info.n_strings, info.n_partials);
    }
    fs::write(out.join("keys.tsv"), keys)?;
    for &key in &notes {
        for &velocity in &velocities {
            let mut script = Vec::new();
            if pedal {
                script.push((0.0, PianoEvent::Sustain { value: 1.0 }));
            }
            script.push((0.0, PianoEvent::NoteOn { key, velocity }));
            if let Some(t) = release {
                script.push((t, PianoEvent::NoteOff { key }));
            }
            let mut p = setup.piano();
            let (l, r) = render(&mut p, rate, &script, seconds);
            wav(&out.join(format!("note_{key:03}_vel_{velocity:03}.wav")), rate as u32, &l, &r)?;
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("low_register: {e}");
        std::process::exit(1);
    }
}
