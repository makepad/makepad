//! Switching an instrument: what a score can play right now, grouped, and
//! the source edit that turns one instrument node into another. The node
//! keeps its name (every loop that plays it keeps playing it, notes and
//! all), its place on the graph, and the settings both instruments share
//! (level, mono, the envelope between synths). One splice, one undo step.

use crate::model::{AssetRef, InstKind, ScoreDoc};
use crate::source::{self, Splice};
use makepad_score_player::presets;

/// The General MIDI families, eight programs each, as a picker names them.
pub const GM_FAMILIES: [&str; 16] = [
    "GM pianos", "GM chromatic percussion", "GM organs", "GM guitars", "GM basses", "GM strings", "GM ensembles", "GM brass",
    "GM reeds", "GM pipes", "GM synth leads", "GM synth pads", "GM synth effects", "GM ethnic", "GM percussive", "GM sound effects",
];

/// The groups a picker shows, in order: the instruments made here, the
/// General MIDI set by family and its kits, the library's and the
/// person's own, then the ways out and in.
pub const GROUPS: [&str; 31] = [
    "My instruments", "Synths", "Morph", "Shader instruments", "Drum kits", "GM drum kits", "Piano", "Ironfish",
    "GM pianos", "GM chromatic percussion", "GM organs", "GM guitars", "GM basses", "GM strings", "GM ensembles", "GM brass",
    "GM reeds", "GM pipes", "GM synth leads", "GM synth pads", "GM synth effects", "GM ethnic", "GM percussive", "GM sound effects",
    "SoundFonts", "Samplers", "Voices", "Neural singers", "Library singers", "Audio in", "MIDI out",
];

/// One instrument a node can become.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub group: &'static str,
    pub label: String,
    /// A short line about it (hover docs).
    pub detail: String,
    /// Its node type and the fields that make it this instrument.
    pub type_name: &'static str,
    pub fields: Vec<(String, String)>,
}

impl Choice {
    /// A stable key (what the picker marks as current).
    pub fn key(&self) -> String {
        let fields: Vec<String> = self.fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{}:{}", self.type_name, fields.join(","))
    }
}

/// Library assets a host knows of beside the score's own: SoundFonts,
/// samples and singers, as (title, asset), and the instruments of the
/// person's own scores (`mine`).
#[derive(Clone, Debug, Default)]
pub struct Available {
    pub fonts: Vec<(String, AssetRef)>,
    pub samples: Vec<(String, AssetRef)>,
    pub singers: Vec<(String, AssetRef)>,
    pub mine: Vec<Choice>,
}

/// The instruments a score defines that are more than a built-in (a synth
/// with its own settings, a shader written for it, a sampler, a tuned
/// voice), as choices of "My instruments": `title` is the score's.
pub fn instruments_of(src: &str, title: &str) -> Vec<Choice> {
    let outline = source::outline(src);
    let mut out = Vec::new();
    for ctor in &outline.nodes {
        let Some(type_name) = INSTRUMENT_TYPES.iter().copied().find(|t| *t == ctor.type_name) else { continue };
        let fields: Vec<(String, String)> = ctor.fields.iter().filter(|f| !f.bare && f.key != "at" && f.key != "solo" && f.key != "mute").map(|f| (f.key.clone(), f.value.text(src).trim().to_string())).collect();
        // Only an identity field (`preset`, `kit`, `kind` …): that is a built-in.
        if fields.len() <= 1 && type_name != "Shader" && type_name != "Sampler" {
            continue;
        }
        let about: String = fields.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join(" ").chars().take(80).collect();
        out.push(Choice { group: "My instruments", label: format!("{} · {title}", ctor.name), detail: about, type_name, fields });
    }
    out
}

/// General MIDI program names.
pub const GM_PROGRAMS: [&str; 128] = [
    "Acoustic Grand", "Bright Piano", "Electric Grand", "Honky-tonk", "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
    "Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
    "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
    "Nylon Guitar", "Steel Guitar", "Jazz Guitar", "Clean Guitar", "Muted Guitar", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
    "Acoustic Bass", "Finger Bass", "Pick Bass", "Fretless Bass", "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
    "Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
    "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2", "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
    "Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
    "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
    "Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
    "Square Lead", "Saw Lead", "Calliope Lead", "Chiff Lead", "Charang Lead", "Voice Lead", "Fifths Lead", "Bass + Lead",
    "New Age Pad", "Warm Pad", "Polysynth Pad", "Choir Pad", "Bowed Pad", "Metallic Pad", "Halo Pad", "Sweep Pad",
    "Rain", "Soundtrack", "Crystal", "Atmosphere", "Brightness", "Goblins", "Echoes", "Sci-fi",
    "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bagpipe", "Fiddle", "Shanai",
    "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
    "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone", "Helicopter", "Applause", "Gunshot",
];

fn quoted(text: &str) -> String {
    format!("{text:?}")
}

/// `@name`, or `"name"` where an id cannot be written (`boom_bass`).
pub fn atom(name: &str) -> String {
    if name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') { format!("@{name}") } else { quoted(name) }
}

fn asset(a: &AssetRef) -> String {
    format!("asset({}, {})", quoted(&a.id), quoted(&a.rev))
}

/// What a score can play right now: the recipe synths, the library's
/// shader instruments, the drum kits, the modelled piano and kit, Ironfish
/// programs, a program of every SoundFont the score or the library has,
/// the samples and the voices.
pub fn catalog(doc: Option<&ScoreDoc>, available: &Available) -> Vec<Choice> {
    let mut out = available.mine.clone();
    let choice = |group, label: String, detail: String, type_name, fields: Vec<(&str, String)>| Choice {
        group, label, detail, type_name, fields: fields.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
    };
    let sounds = crate::palette::sounds::sounds();
    for p in &sounds.patches {
        out.push(choice("Synths", p.name.replace('_', " "), format!("synth patch @{} ({}): {}", p.name, p.family, p.about), "Synth", vec![("preset", atom(&p.name))]));
    }
    for (name, code) in makepad_score_player::shader::LIBRARY {
        let is_instrument = code.contains("fn voice");
        if is_instrument {
            let about = code.lines().next().unwrap_or("").trim_start_matches("//").trim().to_string();
            out.push(choice("Shader instruments", name.replace('_', " "), about, "Shader", vec![("lib", quoted(name))]));
        }
    }
    out.push(choice("Drum kits", "acoustic (modelled)".into(), "the modelled acoustic kit: every General MIDI piece".into(), "Instrument", vec![("kind", "@drums".into())]));
    for kit in &sounds.kits {
        out.push(choice("Drum kits", kit.name.replace('_', " "), format!("synthesized kit @{}: {}", kit.name, kit.about), "Drums", vec![("kit", atom(&kit.name))]));
    }
    for preset in makepad_audio_synth::instruments::PianoInstrument::preset_names() {
        out.push(choice("Piano", preset.to_string(), "the physically modelled grand".into(), "Instrument", vec![("kind", "@piano".into()), ("preset", quoted(preset))]));
    }
    for (i, program) in presets::IRON_PROGRAMS.iter().enumerate() {
        let _ = i;
        out.push(choice("Ironfish", program.to_string(), "Ironfish: 16 voices with its own chorus, delay and reverb".into(), "Ironfish", vec![("program", quoted(program))]));
    }
    for (name, about, _) in makepad_audio_synth::morph::PRESETS {
        out.push(choice("Morph", name.replace('_', " "), format!("Morph wavetable synth: {about}"), "Morph", vec![("preset", atom(name))]));
    }
    out.push(choice("Audio in", "input".into(), "the audio input (a microphone, a line in) as it comes, through the loop's FX".into(), "Instrument", vec![("kind", "@input".into())]));
    // Outboard gear on MIDI channels (every output; `port:` names one).
    for channel in 1..=16 {
        out.push(choice("MIDI out", format!("channel {channel}"), format!("the notes go out as MIDI on channel {channel}, to outboard gear or another app"), "Instrument", vec![
            ("kind", "@midi".into()), ("channel", channel.to_string()),
        ]));
    }
    // The General MIDI SoundFont's programs by family and its kits
    // (installed on demand), then the SoundFonts of the score and the
    // library.
    for (program, name) in GM_PROGRAMS.iter().enumerate() {
        out.push(choice(GM_FAMILIES[program / 8], name.to_string(), format!("General MIDI program {program} of the GM SoundFont (FluidR3 GM)"), "Instrument", vec![
            ("kind", "@soundfont".into()), ("program", program.to_string()),
        ]));
    }
    for entry in crate::palette::entries() {
        if let crate::palette::Kind::GmKit { program } = entry.kind {
            out.push(choice("GM drum kits", entry.name.clone(), format!("General MIDI drum kit {program} of the GM SoundFont: {}", entry.tags.join(" ")), "Instrument", vec![
                ("kind", "@soundfont".into()), ("bank", "128".into()), ("program", program.to_string()),
            ]));
        }
    }
    let mut fonts: Vec<(String, AssetRef)> = available.fonts.clone();
    if let Some(doc) = doc {
        for a in doc.files() {
            if !fonts.iter().any(|(_, f)| *f == a) {
                fonts.push((format!("{}", a.id.chars().take(10).collect::<String>()), a));
            }
        }
    }
    for (title, font) in &fonts {
        for (program, name) in GM_PROGRAMS.iter().enumerate() {
            out.push(choice("SoundFonts", format!("{title} · {program} {name}"), format!("General MIDI program {program} of the SoundFont {title}"), "Instrument", vec![
                ("kind", "@soundfont".into()), ("src", asset(font)), ("program", program.to_string()),
            ]));
        }
    }
    let mut samples = available.samples.clone();
    if let Some(doc) = doc {
        for inst in &doc.insts {
            if let InstKind::Sampler { src, .. } = &inst.kind {
                if !samples.iter().any(|(_, s)| s == src) {
                    samples.push((inst.name.clone(), src.clone()));
                }
            }
        }
    }
    for (title, sample) in &samples {
        out.push(choice("Samplers", title.clone(), "a library sample, pitched from its root".into(), "Sampler", vec![("src", asset(sample))]));
    }
    for voice in ["soprano", "alto", "tenor", "baritone", "bass"] {
        out.push(choice("Voices", voice.to_string(), format!("a sung {voice} voice: sing(notes, \"lyrics\") in its loops"), "Voice", vec![("profile", format!("@{voice}"))]));
    }
    // The neural voice, when one is installed (`cantor_install`).
    #[cfg(not(target_arch = "wasm32"))]
    if makepad_score_player::sung::neural::model_path().is_file() {
        out.push(choice("Neural singers", "Cantor".into(), "the neural singing voice: sing(notes, \"lyrics\") in its loops, each line rendered ahead".into(), "Voice", vec![("engine", "@neural".into())]));
    }
    for (title, singer) in &available.singers {
        out.push(choice("Library singers", title.clone(), "a library singer".into(), "Voice", vec![("profile", asset(singer))]));
    }
    out
}

/// The node types that are instruments.
pub const INSTRUMENT_TYPES: [&str; 8] = ["Synth", "Drums", "Sampler", "Ironfish", "Morph", "Shader", "Voice", "Instrument"];

/// The key of the choice `node` is now, if it is one of them.
pub fn current_key(src: &str, node: &str) -> Option<String> {
    let outline = source::outline(src);
    let ctor = outline.node(node)?;
    let value = |key: &str| ctor.field(key).map(|f| f.value.text(src).trim().to_string());
    let fields: Vec<(String, String)> = match ctor.type_name.as_str() {
        "Synth" => vec![("preset".into(), value("preset")?)],
        "Shader" => vec![("lib".into(), value("lib")?)],
        "Drums" => vec![("kit".into(), value("kit")?)],
        "Ironfish" => vec![("program".into(), value("program")?)],
        "Morph" => vec![("preset".into(), value("preset")?)],
        "Voice" if value("engine").as_deref() == Some("@neural") => vec![("engine".into(), "@neural".into())],
        "Voice" => vec![("profile".into(), value("profile")?)],
        "Sampler" => vec![("src".into(), value("src")?)],
        "Instrument" => {
            let kind = value("kind")?;
            let mut f = vec![("kind".to_string(), kind.clone())];
            match kind.as_str() {
                "@piano" => f.push(("preset".into(), value("preset").unwrap_or_else(|| quoted("Concert Grand")))),
                "@midi" => f.push(("channel".into(), value("channel").unwrap_or_else(|| "1".into()))),
                "@soundfont" => {
                    if let Some(src) = value("src") {
                        f.push(("src".into(), src));
                    }
                    if let Some(bank) = value("bank").filter(|b| b != "0") {
                        f.push(("bank".into(), bank));
                    }
                    // A program by name (`@harp`) is the number it plays.
                    let program = value("program").unwrap_or_else(|| "0".into());
                    let program = program.strip_prefix('@').and_then(crate::gm_font::program).map_or(program.clone(), |p| p.to_string());
                    f.push(("program".into(), program));
                }
                _ => {}
            }
            f
        }
        _ => return None,
    };
    let fields: Vec<String> = fields.iter().map(|(k, v)| format!("{k}={v}")).collect();
    Some(format!("{}:{}", ctor.type_name, fields.join(",")))
}

/// The settings kept across a switch: the graph position, and what both
/// node types read under the same name.
fn kept(from: &str, to: &str, key: &str) -> bool {
    if key == "at" {
        return true;
    }
    let shared: &[&str] = match (from, to) {
        ("Synth", "Synth") => &["attack", "decay", "sustain", "release", "gain", "mono", "unison", "spread", "vib", "vib_depth"],
        ("Synth", "Shader") | ("Shader", "Synth") | ("Shader", "Shader") => &["mono"],
        ("Instrument", "Instrument") => &["gain", "port"],
        ("Voice", "Voice") => &["choir", "parts", "vibrato", "vibrato_rate", "vibrato_delay", "portamento", "scoop", "breath", "drift", "jitter"],
        ("Sampler", "Sampler") => &["root", "attack", "release"],
        ("Morph", "Morph") => &["m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "macro_names", "mono", "legato", "glide", "level", "poly"],
        ("Ironfish", "Ironfish") => &["cutoff", "resonance", "env", "attack", "decay", "sustain", "release", "sub", "noise", "glide", "crush", "delay", "chorus", "reverb", "output"],
        _ => &[],
    };
    shared.contains(&key)
}

/// The edit that makes `node` the instrument `choice`: the node's
/// constructor replaced, its name and every reference to it untouched.
pub fn switch(src: &str, node: &str, choice: &Choice) -> Result<Splice, String> {
    let outline = source::outline(src);
    let ctor = outline.node(node).ok_or_else(|| format!("no node `{node}`"))?;
    if !INSTRUMENT_TYPES.contains(&ctor.type_name.as_str()) {
        return Err(format!("`{node}` is a {}, not an instrument", ctor.type_name));
    }
    let start = ctor.body.start - ctor.type_name.len();
    if src.get(start..ctor.body.start) != Some(ctor.type_name.as_str()) {
        return Err(format!("`{node}`'s type is not written right before its braces"));
    }
    let mut fields: Vec<String> = choice.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    for field in &ctor.fields {
        if field.bare || choice.fields.iter().any(|(k, _)| *k == field.key) {
            continue;
        }
        if kept(&ctor.type_name, choice.type_name, &field.key) {
            fields.push(format!("{}: {}", field.key, field.value.text(src)));
        }
    }
    Ok(Splice { start, end: ctor.body.end, text: format!("{}{{{}}}", choice.type_name, fields.join(" ")) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\n// the bass\nlet clock = Clock{bpm: 120}\nlet bass = Synth{preset: @acid cutoff: 700 attack: 0.01 at: {x: 10 y: 20}}\nlet line = Loop{play: bass beats: 2 code: fn(i) { play(@a1) sleep(1) play(@c2) sleep(1) }}\nScore{clock bass line}\n";

    #[test]
    fn a_voice_switches_to_the_neural_singer_and_back() {
        let src = "use mod.score.*\nlet clock = Clock{bpm: 100}\nlet vox = Voice{profile: @alto vibrato: 0.2}\nlet line = Loop{play: vox beats: 2 code: fn(i) { sleep(sing([@e4 @g4], \"la la\", {dur: 1})) }}\nScore{clock vox line}\n";
        let cantor = Choice { group: "Neural singers", label: "Cantor".into(), detail: String::new(), type_name: "Voice", fields: vec![("engine".into(), "@neural".into())] };
        let neural = source::apply(src, &switch(src, "vox", &cantor).unwrap());
        assert!(neural.contains("let vox = Voice{engine: @neural vibrato: 0.2}"), "{neural}");
        assert_eq!(current_key(&neural, "vox"), Some("Voice:engine=@neural".into()));
        assert!(crate::model::ScoreDoc::load(&neural).is_ok());
        let alto = Choice { group: "Voices", label: "alto".into(), detail: String::new(), type_name: "Voice", fields: vec![("profile".into(), "@alto".into())] };
        let back = source::apply(&neural, &switch(&neural, "vox", &alto).unwrap());
        assert_eq!(back, src);
        assert_eq!(current_key(src, "vox"), Some("Voice:profile=@alto".into()));
    }

    #[test]
    fn a_program_named_is_the_choice_it_plays() {
        let src = "use mod.score.*\nlet harp = Instrument{kind: @soundfont program: @harp}\nlet line = Loop{play: harp beats: 1 code: fn(i) { play(@c4) sleep(1) }}\nScore{harp line}\n";
        let key = current_key(src, "harp").unwrap();
        let all = catalog(None, &Available::default());
        let now = all.iter().find(|c| c.key() == key).unwrap();
        assert_eq!((now.group, now.label.as_str()), ("GM strings", "Orchestral Harp"));
        let guitar = all.iter().find(|c| c.label == "Nylon Guitar").unwrap();
        let next = source::apply(src, &switch(src, "harp", guitar).unwrap());
        assert!(next.contains("let harp = Instrument{kind: @soundfont program: 24}"), "{next}");
        assert_eq!(current_key(&next, "harp"), Some(guitar.key()));
    }

    #[test]
    fn switching_rewrites_one_node_and_keeps_the_loop() {
        let doc = crate::model::ScoreDoc::load(SRC).unwrap();
        let all = catalog(Some(&doc), &Available::default());
        for group in GROUPS.iter().filter(|g| !["My instruments", "SoundFonts", "Samplers", "Neural singers", "Library singers"].contains(g)) {
            assert!(all.iter().any(|c| c.group == *group), "no {group}");
        }
        assert_eq!(current_key(SRC, "bass"), Some("Synth:preset=@acid".into()));
        // To the piano: the loop and everything else stay byte for byte.
        let piano = all.iter().find(|c| c.group == "Piano").unwrap();
        let next = source::apply(SRC, &switch(SRC, "bass", piano).unwrap());
        assert!(next.contains("let bass = Instrument{kind: @piano preset: \"Concert Grand\" at: {x: 10 y: 20}}"), "{next}");
        assert_eq!(next.replace(&next[next.find("Instrument{").unwrap()..next.find("}\nlet line").unwrap() + 1], "X"), SRC.replace(&SRC[SRC.find("Synth{").unwrap()..SRC.find("}\nlet line").unwrap() + 1], "X"));
        assert!(crate::model::ScoreDoc::load(&next).is_ok());
        // Synth to synth keeps the envelope, drops the old preset's filter.
        let pad = all.iter().find(|c| c.label == "warm pad").unwrap();
        let next = source::apply(SRC, &switch(SRC, "bass", pad).unwrap());
        assert!(next.contains("Synth{preset: @warm_pad attack: 0.01 at: {x: 10 y: 20}}"), "{next}");
        // Every choice makes a score that loads (the SoundFonts need a file).
        for c in all.iter() {
            let next = source::apply(SRC, &switch(SRC, "bass", c).unwrap());
            if let Err(d) = crate::model::ScoreDoc::load(&next) {
                panic!("{}: {d:?}\n{next}", c.label);
            }
        }
        assert!(switch(SRC, "line", pad).is_err());
    }
}
