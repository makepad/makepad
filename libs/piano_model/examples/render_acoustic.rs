//! Offline held-note renderer. See tools/README.md; no audio device or workers.
use makepad_piano_model::{
    calibration::{CalibrationNote, CALIBRATION_PARTIALS},
    DesignParams, Piano, PianoEvent, TimedEvent, Voicing,
};
use std::{env, error::Error, fs, io::Write, path::PathBuf};

const NOTES: &str = "21,24,30,33,36,45,48,60,69,72,84,96";
fn help() -> String {
    format!("render_acoustic --out DIR [--rate 48000] [--notes 21,24,...] \
    [--velocities 28,68,112] [--seconds 4] [--stock | --raw] \
    [--design name=value,...] [--calibration FILE.csv] [--dry] \
    [--voicing name=value,...]\n\
    DIR must not exist. Each note uses a fresh Piano, NoteOn at sample zero,\n\
    key held throughout; constructor effects are retained unless --dry.\n\
    --dry disables reverb, early reflections and limiter/soft clipping;\n\
    modelled soundboard radiation is retained in every mode.\n\
    --design requires --raw; see DesignParams::NAMES in src/params.rs.\n\
    --calibration replaces stock calibration (also with --stock); conflicts\n\
    with --raw/--design. CSV header: key,partial,pp_db,mf_db,ff_db,decay_scale\n\
    Keys 21..108 must be strictly increasing in groups of {CALIBRATION_PARTIALS} rows, with\n\
    partials 1..{CALIBRATION_PARTIALS} exactly once per key; finite gains -36..24 dB, decay 0.1..4.\n\
    Incomplete groups (including legacy 64-row groups) fail before output creation.\n\
    --voicing overrides Voicing::default in every mode: body_tap, knock,\n\
    roughness, phantoms, attack_noise, attack_body, sympathetic. Values must\n\
    be finite 0..2.5, except attack_body 0..1.\n\
    render.json embeds effective voicing values and the CSV path/contents.")
}

fn midi_list(text: &str, lo: u8, hi: u8) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut result = Vec::new();
    for part in text.split(',') {
        let value: u8 = part.parse()?;
        if !(lo..=hi).contains(&value) || result.contains(&value) {
            return Err(format!("invalid or duplicate MIDI value {value}").into());
        }
        result.push(value);
    }
    Ok(result)
}

fn calibration_csv(csv: &str) -> Result<Vec<CalibrationNote>, Box<dyn Error>> {
    let mut lines = csv.lines();
    if lines.next() != Some("key,partial,pp_db,mf_db,ff_db,decay_scale") {
        return Err("expected CSV header key,partial,pp_db,mf_db,ff_db,decay_scale".into());
    }
    let rows: Vec<_> = lines.collect();
    if rows.is_empty() || rows.len() % CALIBRATION_PARTIALS != 0 {
        return Err(format!("calibration CSV requires exactly {CALIBRATION_PARTIALS} rows per key and at least one key").into());
    }
    let mut notes: Vec<CalibrationNote> = Vec::new();
    for (group, rows) in rows.chunks_exact(CALIBRATION_PARTIALS).enumerate() {
        let mut note = CalibrationNote {
            key: 0,
            gain_db: [[0.0; CALIBRATION_PARTIALS]; 3],
            decay_scale: [1.0; CALIBRATION_PARTIALS],
        };
        let mut seen = [false; CALIBRATION_PARTIALS];
        for (row, line) in rows.iter().enumerate() {
            let fields: Vec<_> = line.split(',').collect();
            let line_no = group * CALIBRATION_PARTIALS + row + 2;
            if fields.len() != 6 {
                return Err(format!("CSV line {line_no}: expected six fields").into());
            }
            let key: u8 = fields[0].parse()?;
            let partial: usize = fields[1].parse()?;
            if row == 0 {
                if !(21..=108).contains(&key) || notes.last().is_some_and(|prev| prev.key >= key) {
                    return Err(format!("CSV line {line_no}: keys must increase strictly within 21..108").into());
                }
                note.key = key;
            }
            if key != note.key || !(1..=CALIBRATION_PARTIALS).contains(&partial) || seen[partial - 1] {
                return Err(format!("CSV line {line_no}: each key needs partials 1..{CALIBRATION_PARTIALS} exactly once").into());
            }
            seen[partial - 1] = true;
            for (column, field) in fields[2..].iter().enumerate() {
                let value: f32 = field.parse()?;
                let bounds = if column < 3 { -36.0..=24.0 } else { 0.1..=4.0 };
                if !value.is_finite() || !bounds.contains(&value) {
                    return Err(format!("CSV line {line_no}: gains must be finite -36..24 dB; decay finite 0.1..4").into());
                }
                if column < 3 {
                    note.gain_db[column][partial - 1] = value;
                } else {
                    note.decay_scale[partial - 1] = value;
                }
            }
        }
        notes.push(note);
    }
    Ok(notes)
}

fn json_string(text: &str) -> String {
    let mut result = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            ch if ch < '\u{20}' => result.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => result.push(ch),
        }
    }
    result.push('"');
    result
}

fn wav(path: PathBuf, rate: u32, l: &[f32], r: &[f32]) -> Result<(), Box<dyn Error>> {
    let size = u32::try_from(l.len() * 8)?;
    let mut out = std::io::BufWriter::new(fs::OpenOptions::new().write(true).create_new(true).open(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(size + 50).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&18u32.to_le_bytes())?;
    out.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    out.write_all(&2u16.to_le_bytes())?;
    out.write_all(&rate.to_le_bytes())?;
    out.write_all(&(rate * 8).to_le_bytes())?;
    out.write_all(&8u16.to_le_bytes())?;
    out.write_all(&32u16.to_le_bytes())?;
    out.write_all(&0u16.to_le_bytes())?; // cbSize
    out.write_all(b"fact")?;
    out.write_all(&4u32.to_le_bytes())?;
    out.write_all(&(l.len() as u32).to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&size.to_le_bytes())?;
    for (&left, &right) in l.iter().zip(r) {
        if !left.is_finite() || !right.is_finite() {
            return Err("non-finite model output".into());
        }
        out.write_all(&left.to_le_bytes())?;
        out.write_all(&right.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), Box<dyn Error>> {
    let mut out = None;
    let mut rate = 48000u32;
    let mut seconds = 4.0f64;
    let mut notes = midi_list(NOTES, 21, 108)?;
    let mut velocities = midi_list("28,68,112", 1, 127)?;
    let mut mode = None;
    let mut design = None;
    let mut calibration_path = None;
    let mut voicing_overrides = None;
    let mut dry = false;
    while let Some(arg) = args.next() {
        if arg == "--help" || arg == "-h" {
            println!("{}", help());
            return Ok(());
        }
        if arg == "--stock" || arg == "--raw" {
            if mode.replace(arg == "--raw").is_some() {
                return Err("choose --stock or --raw once".into());
            }
            continue;
        }
        if arg == "--dry" {
            dry = true;
            continue;
        }
        let value = args.next().ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(value)),
            "--rate" => rate = value.parse()?,
            "--seconds" => seconds = value.parse()?,
            "--notes" => notes = midi_list(&value, 21, 108)?,
            "--velocities" => velocities = midi_list(&value, 1, 127)?,
            "--design" => design = Some(value),
            "--voicing" => {
                if voicing_overrides.replace(value).is_some() {
                    return Err("choose --voicing once".into());
                }
            }
            "--calibration" => {
                if calibration_path.replace(value).is_some() {
                    return Err("choose --calibration once".into());
                }
            }
            _ => return Err(format!("unknown option {arg}").into()),
        }
    }
    let raw = mode.unwrap_or(false);
    if calibration_path.is_some() && (raw || design.is_some()) {
        return Err("--calibration conflicts with --raw/--design".into());
    }
    let out = out.ok_or("--out is required")?;
    if !(16000..=192000).contains(&rate) || !seconds.is_finite() || !(2.1..=60.0).contains(&seconds) {
        return Err("rate must be 16000..192000 Hz; seconds must be 2.1..60".into());
    }
    let mut params = DesignParams::default();
    if let Some(design) = design {
        if !raw {
            return Err("--design requires --raw".into());
        }
        for pair in design.split(',') {
            let (name, value) = pair.split_once('=').ok_or("expected --design name=value,...")?;
            let value: f64 = value.parse()?;
            if !value.is_finite() || !params.set(name, value) {
                return Err(format!("unknown parameter or non-finite value: {pair}").into());
            }
        }
    }
    let mut voicing = Voicing::default();
    if let Some(overrides) = voicing_overrides {
        for pair in overrides.split(',') {
            let (name, value) = pair.split_once('=').ok_or("expected --voicing name=value,...")?;
            let field = match name {
                "body_tap" => &mut voicing.body_tap,
                "knock" => &mut voicing.knock,
                "roughness" => &mut voicing.roughness,
                "phantoms" => &mut voicing.phantoms,
                "attack_noise" => &mut voicing.attack_noise,
                "attack_body" => &mut voicing.attack_body,
                "sympathetic" => &mut voicing.sympathetic,
                _ => return Err(format!("unknown voicing field: {name}").into()),
            };
            // Validate before narrowing so rounding cannot admit out-of-range inputs.
            let value: f64 = value.parse()?;
            let max = if name == "attack_body" { 1.0 } else { 2.5 };
            if !value.is_finite() || !(0.0..=max).contains(&value) {
                return Err(format!("voicing {name} must be finite 0..{max}: {pair}").into());
            }
            *field = value as f32;
        }
    }
    let calibration = calibration_path.as_ref().map(|path| {
        let csv = fs::read_to_string(path)?;
        let table = calibration_csv(&csv)?;
        Ok::<_, Box<dyn Error>>((csv, table))
    }).transpose()?;
    let new_piano = || {
        let mut piano = if let Some((_, table)) = &calibration {
            Piano::new_with_calibration(rate as f32, table)
        } else if raw {
            Piano::new_with_params(rate as f32, &params)
        } else {
            Piano::new(rate as f32)
        };
        piano.set_voicing(voicing);
        if dry {
            piano.set_reverb_mix(0.0);
            piano.set_early_reflection_level(0.0);
            piano.set_soft_clip(false);
        }
        piano
    };
    // Construction validates calibration bounds before any output is created.
    let first_piano = new_piano();
    let effective_voicing = first_piano.voicing();
    let mut first_piano = Some(first_piano);
    // Refuse reuse, even if filenames differ: a baseline directory is immutable.
    fs::create_dir(&out)?;
    let total = (rate as f64 * seconds).round() as usize;
    for &key in &notes {
        for &velocity in &velocities {
            let mut piano = first_piano.take().unwrap_or_else(&new_piano);
            let mut l = vec![0.0; total];
            let mut r = vec![0.0; total];
            let event = [TimedEvent { offset: 0, event: PianoEvent::NoteOn { key, velocity } }];
            for start in (0..total).step_by(256) {
                let end = (start + 256).min(total);
                piano.process(if start == 0 { &event } else { &[] }, &mut l[start..end], &mut r[start..end]);
            }
            let name = format!("note_{key:03}_vel_{velocity:03}.wav");
            wav(out.join(&name), rate, &l, &r)?;
            println!("{name}");
        }
    }
    let parameters = DesignParams::NAMES.iter().map(|name| {
        format!("    \"{name}\": {}", params.get(name).unwrap())
    }).collect::<Vec<_>>().join(",\n");
    let voicing_json = [
        ("body_tap", effective_voicing.body_tap),
        ("knock", effective_voicing.knock),
        ("roughness", effective_voicing.roughness),
        ("phantoms", effective_voicing.phantoms),
        ("attack_noise", effective_voicing.attack_noise),
        ("attack_body", effective_voicing.attack_body),
        ("sympathetic", effective_voicing.sympathetic),
    ].iter().map(|(name, value)| {
        // Widen exactly before formatting, preserving the effective f32 in JSON.
        format!("    \"{name}\": {:?}", f64::from(*value))
    }).collect::<Vec<_>>().join(",\n");
    let mode = if calibration.is_some() { "calibration" } else if raw { "raw" } else { "stock" };
    let effects = if dry {
        "dry: reverb_mix=0, early_reflection_level=0, soft_clip=false (limiter bypassed); modelled soundboard radiation retained"
    } else {
        "constructor defaults"
    };
    let calibration_json = match (&calibration_path, &calibration) {
        (Some(path), Some((csv, _))) => format!("{{\"path\": {}, \"csv\": {}}}", json_string(path), json_string(csv)),
        _ => "null".to_string(),
    };
    fs::write(out.join("render.json"), format!(
        "{{\n  \"schema\": 1,\n  \"mode\": \"{mode}\",\n  \"rate_hz\": {rate},\n  \"seconds\": {seconds},\n  \"notes\": {notes:?},\n  \"velocities\": {velocities:?},\n  \"note_on_sample\": 0,\n  \"note_off\": null,\n  \"block_frames\": 256,\n  \"dry\": {dry},\n  \"effects\": \"{effects}\",\n  \"calibration\": {calibration_json},\n  \"voicing\": {{\n{voicing_json}\n  }},\n  \"raw_api\": \"Piano::new_with_params\",\n  \"design_defaults_plus_overrides\": {{\n{parameters}\n  }}\n}}\n"
    ))?;
    Ok(())
}

fn main() {
    if let Err(error) = run(env::args().skip(1)) {
        eprintln!("render_acoustic: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn csv(keys: &[u8], partials: &[usize]) -> String {
        let mut csv = String::from("key,partial,pp_db,mf_db,ff_db,decay_scale\n");
        for key in keys {
            for partial in partials {
                let db = -(*partial as f32) / 16.0;
                let scale = 0.5 + *partial as f32 / 128.0;
                csv.push_str(&format!("{key},{partial},{db},{db},{db},{scale}\n"));
            }
        }
        csv
    }

    #[test]
    fn csv_preserves_every_partial_in_any_order() {
        let partials: Vec<_> = (1..=CALIBRATION_PARTIALS).rev().collect();
        let notes = calibration_csv(&csv(&[21, 108], &partials)).unwrap();
        assert_eq!(notes.len(), 2);
        for (note, key) in notes.iter().zip([21, 108]) {
            assert_eq!(note.key, key);
            for partial in partials.iter().copied() {
                for gains in note.gain_db {
                    assert_eq!(gains[partial - 1], -(partial as f32) / 16.0);
                }
                assert_eq!(note.decay_scale[partial - 1], 0.5 + partial as f32 / 128.0);
            }
        }
        assert!(help().contains(&format!("partials 1..{CALIBRATION_PARTIALS} exactly once")));
    }

    #[test]
    fn csv_rejects_incomplete_duplicate_and_out_of_range_groups() {
        for count in [0, 64, CALIBRATION_PARTIALS - 1, CALIBRATION_PARTIALS + 1] {
            let error = calibration_csv(&csv(&[21], &(1..=count).collect::<Vec<_>>())).unwrap_err();
            assert!(error.to_string().contains(&format!("exactly {CALIBRATION_PARTIALS} rows per key")));
        }
        // Fifteen legacy groups total a multiple of the new row count,
        // but none is a complete key and groups must not bleed together.
        assert!(calibration_csv(&csv(&(21..36).collect::<Vec<_>>(), &(1..=64).collect::<Vec<_>>())).is_err());
        for invalid in [0, 1, CALIBRATION_PARTIALS + 1] {
            let mut partials: Vec<_> = (1..=CALIBRATION_PARTIALS).collect();
            partials[CALIBRATION_PARTIALS - 1] = invalid;
            let error = calibration_csv(&csv(&[21], &partials)).unwrap_err();
            assert!(error.to_string().contains(&format!("partials 1..{CALIBRATION_PARTIALS} exactly once")));
        }
    }

    #[test]
    fn legacy_csv_fails_before_output_creation() {
        let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target").join(format!("calibration-csv-test-{}", std::process::id()));
        fs::create_dir_all(&scratch).unwrap();
        let input = scratch.join("legacy.csv");
        let output = scratch.join("output");
        fs::write(&input, csv(&[21], &(1..=64).collect::<Vec<_>>())).unwrap();
        let result = run([
            "--out".to_string(), output.to_str().unwrap().to_string(),
            "--calibration".to_string(), input.to_str().unwrap().to_string(),
        ].into_iter());
        let output_created = output.exists();
        fs::remove_dir_all(&scratch).unwrap();
        assert!(result.unwrap_err().to_string().contains(&format!("exactly {CALIBRATION_PARTIALS} rows per key")));
        assert!(!output_created, "invalid calibration must not create output");
    }
}
