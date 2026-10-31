//! The sound palette's offline tools: list the GM SoundFont's presets,
//! render the audition sheet, measure every instrument into the catalogue.
//!
//! `score_palette presets`: the SoundFont's banks, programs and names.
//! `score_palette audition <dir> [id prefix]`: a phrase per program and patch and a groove per kit, as WAVs (`index.tsv` lists them).
//! `score_palette measure <catalogue.tsv> [pieces.tsv]`: a note per program and patch, the kits' grooves and pieces, measured; the gate's faults printed.

use makepad_audio_synth::instruments::{build, InstrumentKind};
use makepad_audio_synth::Instrument;
use makepad_score_live::palette::{self, Entry, Features, Kind};
use std::path::PathBuf;
use std::sync::Arc;

const RATE: f32 = 48_000.0;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("presets") => presets(),
        Some("search") if args.len() >= 2 => {
            for hit in palette::search(&args[1..].join(" "), None, None, 6) {
                println!("{:5.1}  {:<12} {}\n        {}", hit.score, hit.entry.id, hit.entry.code(), hit.entry.about());
            }
            Ok(())
        }
        Some("zones") if args.len() >= 3 => zones(args[1].parse().unwrap_or(0), args[2].parse().unwrap_or(0)),
        Some("audition") if args.len() >= 2 => audition(PathBuf::from(&args[1]), args.get(2).map(String::as_str)),
        Some("measure") if args.len() >= 2 => measure_all(PathBuf::from(&args[1]), args.get(2).map(PathBuf::from)),
        _ => Err("usage: score_palette presets | zones <bank> <program> | search <words> | audition <dir> [id prefix] | measure <catalogue.tsv> [pieces.tsv]".into()),
    };
    if let Err(e) = result {
        eprintln!("score_palette: {e}");
        std::process::exit(1);
    }
}

fn font_bytes() -> Result<Arc<[u8]>, String> {
    makepad_score_live::gm_font::ensure()?;
    makepad_audio_engine::feed::files::get(makepad_score_live::gm_font::FONT).ok_or_else(makepad_score_live::gm_font::not_installed)
}

fn font() -> Result<makepad_soundfont::SoundFont, String> {
    let bytes = font_bytes()?;
    let limits = makepad_soundfont::ParseLimits { max_sample_points: 256 << 20, ..Default::default() };
    makepad_soundfont::parse_sf2_with_limits(&bytes, limits).map_err(|e| e.to_string())
}

fn presets() -> Result<(), String> {
    let font = font()?;
    let mut list: Vec<_> = font.presets.iter().map(|p| (p.bank, p.program, p.name.clone())).collect();
    list.sort();
    for (bank, program, name) in list {
        println!("{bank}\t{program}\t{name}");
    }
    Ok(())
}

/// The resolved zones of one preset (key range, gain, envelope, filter): why a program sounds as it does.
fn zones(bank: u16, program: u16) -> Result<(), String> {
    let font = font()?;
    for z in font.zones.iter().filter(|z| z.bank == bank && z.program == program) {
        let p = &z.parameters;
        println!(
            "keys {:3}-{:3} vel {:3}-{:3} root {:5.1} gain {:6.3} ({:5.1} dB) env d{:.3} a{:.3} h{:.3} d{:.3} s{:.3} r{:.3} cutoff {:7.0} res {:4.1} pan {:5.2} loop {:?} excl {}",
            z.key_range.low, z.key_range.high, z.velocity_range.low, z.velocity_range.high, p.root_key, p.gain, 20.0 * p.gain.max(1e-9).log10(),
            p.envelope.delay, p.envelope.attack, p.envelope.hold, p.envelope.decay, p.envelope.sustain, p.envelope.release,
            p.filter_cutoff_hz, p.filter_resonance_db, p.pan, p.loop_mode, p.exclusive_class
        );
        if let makepad_soundfont::VoiceSource::Sample { sample_id } = p.source {
            use makepad_soundfont::{SampleRead, SampleSource};
            let (a, b) = if p.loop_end > p.loop_start { (p.loop_start, p.loop_end) } else { (p.start_frame, p.end_frame) };
            let (mut sum, mut sq, mut n) = (0.0f64, 0.0f64, 0.0f64);
            for i in a..b {
                if let SampleRead::Resident { left, .. } = font.read_frame(sample_id, i) {
                    sum += left as f64;
                    sq += (left as f64).powi(2);
                    n += 1.0;
                }
            }
            if n > 0.0 {
                println!("    sample {sample_id}: frames {a}..{b} mean/rms {:.3} rms {:.3} mod {:?}", (sum / n) / (sq / n).sqrt().max(1e-9), (sq / n).sqrt(), p.modulation);
            }
        }
    }
    Ok(())
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn file_name(e: &Entry) -> String {
    match e.kind {
        Kind::Gm { bank, program } => format!("gm/{program:03}-b{bank}-{}.wav", slug(&e.name)),
        Kind::GmKit { program } => format!("gm_kits/{program:03}-{}.wav", slug(&e.name)),
        Kind::Kit { name } => format!("kits/{name}.wav"),
        Kind::Patch { name } => format!("patches/{}-{name}.wav", e.family),
    }
}

/// One instrument built straight from the font (no score), kept for the
/// font's parse to be shared by every build while it lives.
fn instrument(bytes: &Arc<[u8]>, e: &Entry) -> Result<Box<dyn Instrument>, String> {
    let sounds = palette::sounds::sounds();
    let (bank, program) = match e.kind {
        Kind::Gm { bank, program } => (bank as u16, program),
        Kind::GmKit { program } => (128, program),
        Kind::Patch { name } => return build(&InstrumentKind::Synth { patch: sounds.patch(name).ok_or("no patch")?.recipe }, RATE),
        Kind::Kit { name } => return Ok(Box::new(KitPlayer::new(sounds.kit(name).ok_or("no kit")?.slots.clone()))),
    };
    build(&InstrumentKind::SoundFont { bytes: bytes.clone(), bank, program }, RATE)
}

/// A synthesized kit as an instrument: General MIDI drum notes onto its
/// slots (as a Drums loop maps them), a voice per hit.
struct KitPlayer {
    slots: Vec<Option<makepad_audio_synth::recipe::Recipe>>,
    voices: Vec<makepad_audio_synth::recipe::RecipeVoice>,
    seed: u32,
}

impl KitPlayer {
    fn new(slots: Vec<Option<makepad_audio_synth::recipe::Recipe>>) -> Self {
        KitPlayer { slots, voices: Vec::new(), seed: 1 }
    }
}

/// The slot a General MIDI drum note plays (the loop engine's map).
fn slot_of_note(note: u8) -> Option<usize> {
    use makepad_score_player::presets::{gm_drum, DRUM_SLOTS};
    DRUM_SLOTS.iter().position(|s| gm_drum(s) == Some(note)).or(match note {
        35 => Some(0), 40 => Some(1), 44 => Some(3), 41 | 43 => Some(6), 45 | 47 => Some(7), 48 | 50 => Some(8),
        52 | 55 | 57 => Some(9), 51 | 53 | 59 => Some(10), 54 | 69 | 70 | 82 => Some(11), 56 => Some(12),
        60..=68 => Some(13), 75 | 76 | 77 => Some(5), _ => None,
    })
}

impl Instrument for KitPlayer {
    fn name(&self) -> &str {
        "kit"
    }
    fn sample_rate(&self) -> f32 {
        RATE
    }
    fn note_on(&mut self, note: u8, velocity: u8) {
        let Some(Some(recipe)) = slot_of_note(note).and_then(|s| self.slots.get(s)) else { return };
        let mut v = makepad_audio_synth::recipe::RecipeVoice::new(recipe, 1.0, None, false, self.seed);
        v.recipe.volume *= (velocity as f32 / 127.0).powi(2);
        self.seed += 1;
        self.voices.push(v);
    }
    fn note_off(&mut self, _note: u8) {}
    fn all_notes_off(&mut self) {
        self.voices.clear();
    }
    fn render(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let mut mono = vec![0.0f32; out_l.len()];
        for v in &mut self.voices {
            v.render(&mut mono, RATE, 1.0);
        }
        self.voices.retain(|v| !v.finished());
        for i in 0..out_l.len() {
            out_l[i] += mono[i];
            out_r[i] += mono[i];
        }
    }
}

fn audition(dir: PathBuf, filter: Option<&str>) -> Result<(), String> {
    let bytes = font_bytes()?;
    // Holds the parsed font for every render's graph.
    let _keep = instrument(&bytes, &palette::entries()[0])?;
    for sub in ["gm", "gm_kits", "kits", "patches"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(|e| e.to_string())?;
    }
    let mut index = String::from("file\tid\tname\tcode\tabout\n");
    for e in palette::entries() {
        if filter.is_some_and(|f| !e.id.starts_with(f)) {
            continue;
        }
        let source = palette::audition_score(&e);
        let seconds = if e.phrase == palette::Phrase::Groove { 8.6 } else { 6.8 };
        let opts = makepad_score_live::RenderOpts { rate: RATE, seconds: Some(seconds), ..Default::default() };
        let r = makepad_score_live::render(&source, &opts, &makepad_score_live::NoSamples).map_err(|d| format!("{}: {d:?}", e.id))?;
        let name = file_name(&e);
        std::fs::write(dir.join(&name), makepad_audio_synth::wav::stereo_wav_16(&r.left, &r.right, RATE as u32)).map_err(|e| e.to_string())?;
        let f = palette::measure(&r.left, &r.right, RATE);
        eprintln!("{:<12} {:<28} {:6.1} LUFS peak {:5.1}", e.id, e.name, f.lufs, f.peak_db);
        index.push_str(&format!("{name}\t{}\t{}\t{}\t{}\n", e.id, e.name, e.code(), e.about()));
    }
    std::fs::write(dir.join("index.tsv"), index).map_err(|e| e.to_string())?;
    Ok(())
}

/// Render notes (seconds, note, held seconds, velocity) through an instrument.
fn play(inst: &mut dyn Instrument, notes: &[(f32, u8, f32, u8)], seconds: f32) -> (Vec<f32>, Vec<f32>) {
    let total = (seconds * RATE) as usize;
    let (mut l, mut r) = (vec![0.0f32; total], vec![0.0f32; total]);
    let mut events: Vec<(usize, u8, Option<u8>)> = Vec::new();
    for (t, note, held, vel) in notes {
        events.push(((t * RATE) as usize, *note, Some(*vel)));
        events.push((((t + held) * RATE) as usize, *note, None));
    }
    events.sort_by_key(|e| (e.0, e.2.is_some()));
    let mut next = 0;
    for start in (0..total).step_by(64) {
        while next < events.len() && events[next].0 < start + 64 {
            match events[next].2 {
                Some(vel) => inst.note_on(events[next].1, vel),
                None => inst.note_off(events[next].1),
            }
            next += 1;
        }
        let end = (start + 64).min(total);
        inst.render(&mut l[start..end], &mut r[start..end]);
    }
    (l, r)
}

fn measure_all(out: PathBuf, pieces: Option<PathBuf>) -> Result<(), String> {
    let bytes = font_bytes()?;
    let entries = palette::entries();
    let mut measured: Vec<(Entry, Features)> = Vec::new();
    let mut piece_rows = String::from("# kit\tnote\tfeatures...\n");
    let mut pieces_of: Vec<(String, u8, Features)> = Vec::new();
    for e in &entries {
        let mut inst = instrument(&bytes, e)?;
        let f = match e.kind {
            Kind::Gm { .. } | Kind::Patch { .. } => {
                let (l, r) = play(inst.as_mut(), &[(0.0, e.root, 1.5, 100)], 3.0);
                let mut f = palette::measure(&l, &r, RATE);
                // Harshness and DC over the audition phrase: many notes, a smooth spectrum.
                let mut again = instrument(&bytes, e)?;
                let notes: Vec<(f32, u8, f32, u8)> = palette::audition_notes(e).iter().map(|(b, n, d, v)| (b * 0.6, *n, d * 0.6, (v * 127.0) as u8)).collect();
                let (pl, pr) = play(again.as_mut(), &notes, 6.8);
                let phrase = palette::measure(&pl, &pr, RATE);
                f.harsh_db = phrase.harsh_db;
                f.dc = phrase.dc;
                f
            }
            Kind::GmKit { .. } | Kind::Kit { .. } => {
                let mut loudest = -120.0f32;
                for note in 35..=81u8 {
                    let mut one = instrument(&bytes, e)?;
                    let (l, r) = play(one.as_mut(), &[(0.0, note, 0.15, 100)], 2.5);
                    let f = palette::measure(&l, &r, RATE);
                    loudest = loudest.max(f.peak_db);
                    pieces_of.push((e.id.clone(), note, f));
                    let v: Vec<String> = f.values().iter().map(|x| format!("{x:.3}")).collect();
                    piece_rows.push_str(&format!("{}\t{note}\t{}\n", e.id, v.join("\t")));
                }
                // The groove at 100 bpm, through the instrument.
                let notes: Vec<(f32, u8, f32, u8)> = palette::audition_notes(e).iter().map(|(b, n, d, v)| (b * 0.6, *n, d * 0.6, (v * 127.0) as u8)).collect();
                let (l, r) = play(inst.as_mut(), &notes, 8.6);
                // A kit's peak is its loudest piece's, not the groove's sum.
                Features { peak_db: loudest, ..palette::measure(&l, &r, RATE) }
            }
        };
        // The gate is for the library's own sounds; a GM program's faults are the font's (reported, not gated).
        let faults = if matches!(e.kind, Kind::Kit { .. } | Kind::Patch { .. }) { f.faults() } else { Vec::new() };
        eprintln!("{:<12} {:<28} centroid {:6.0} attack {:5.1} decay {:6.0} low {:.2} high {:.2} {:6.1} LUFS harsh {:5.1}{}", e.id, e.name, f.centroid, f.attack_ms, f.decay_ms, f.low, f.high, f.lufs, f.harsh_db, if faults.is_empty() { String::new() } else { format!("  FAULT: {}", faults.join(", ")) });
        measured.push((e.clone(), f));
    }
    let mut text = String::from("# Measured by `score_palette measure` (do not edit): id, the features (see features.rs COLUMNS), the words they earn.\n");
    for (e, f) in &measured {
        let peers: Vec<Features> = measured.iter().filter(|(p, _)| p.family == e.family).map(|(_, f)| *f).collect();
        let mut words = palette::describe(f, &peers, e.kind.is_kit());
        if e.kind.is_kit() {
            // The pieces that make a kit's character, ranked among the kits.
            let piece = |id: &str, note: u8| pieces_of.iter().find(|(k, n, _)| k == id && *n == note).map(|(_, _, f)| *f);
            for (note, what, key, lo, hi) in [
                (36u8, "kick", (|f: &Features| f.decay_ms) as fn(&Features) -> f32, "tight_kick", "boomy_kick"),
                (38, "snare", |f: &Features| f.centroid, "dark_snare", "bright_snare"),
                (38, "snare", |f: &Features| f.decay_ms, "tight_snare", "long_snare"),
                (42, "hats", |f: &Features| f.centroid, "dark_hats", "bright_hats"),
                (49, "crash", |f: &Features| f.decay_ms, "short_crash", "long_crash"),
            ] {
                let _ = what;
                let Some(mine) = piece(&e.id, note) else { continue };
                let all: Vec<f32> = measured.iter().filter_map(|(k, _)| piece(&k.id, note)).map(|f| key(&f)).collect();
                let rank = all.iter().filter(|v| **v < key(&mine)).count() as f32 / (all.len().max(2) - 1) as f32;
                let mut sorted = all.clone();
                sorted.sort_by(f32::total_cmp);
                let median = sorted[sorted.len() / 2];
                // Kits share pieces: a rank among equals says nothing.
                if rank <= 0.25 && key(&mine) < median * 0.9 {
                    words.push(lo.into());
                } else if rank >= 0.75 && key(&mine) > median * 1.1 {
                    words.push(hi.into());
                }
            }
        }
        let v: Vec<String> = f.values().iter().map(|x| format!("{x:.3}")).collect();
        text.push_str(&format!("{}\t{}\t{}\n", e.id, v.join("\t"), words.join(" ")));
    }
    std::fs::write(&out, text).map_err(|e| e.to_string())?;
    if let Some(p) = pieces {
        std::fs::write(p, piece_rows).map_err(|e| e.to_string())?;
    }
    Ok(())
}
