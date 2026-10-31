//! The sound palette: every instrument a score can name, with tags and
//! features measured offline, and the search the AI tools offer
//! (`instrument_search`).
//!
//! - `gm.tsv` / `gm_kits.tsv`: the General MIDI SoundFont's presets and
//!   drum kits, what each is for (family, roles, the register it plays
//!   in, tags).
//! - `catalogue.tsv`: the features `score_palette measure-gm` measured
//!   from each one (a note or a kit's pieces, rendered offline), and the
//!   words they earn (bright, dark, punchy, long ...).
//! - [`search`]: a description ("thin bright 80s drum machine", "warm
//!   upright bass") to the instruments that fit, best first, each with
//!   the Score code that plays it.

pub mod features;
pub mod sounds;

pub use features::{measure, Features};

const GM: &str = include_str!("gm.tsv");
const GM_KITS: &str = include_str!("gm_kits.tsv");
const CATALOGUE: &str = include_str!("catalogue.tsv");

/// What kind of instrument an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A General MIDI SoundFont preset (bank, program).
    Gm { bank: u8, program: u8 },
    /// A General MIDI SoundFont drum kit (bank 128, program).
    GmKit { program: u8 },
    /// A synthesized kit of the sound library (`Drums{kit: @name}`).
    Kit { name: &'static str },
    /// A synth patch of the sound library (`Synth{preset: @name}`).
    Patch { name: &'static str },
}

impl Kind {
    pub fn is_kit(&self) -> bool {
        matches!(self, Kind::GmKit { .. } | Kind::Kit { .. })
    }
}

/// How an audition plays an instrument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phrase {
    Melody,
    Chords,
    Bass,
    Hits,
    Single,
    Groove,
}

impl Phrase {
    fn parse(s: &str) -> Phrase {
        match s {
            "chords" => Phrase::Chords,
            "bass" => Phrase::Bass,
            "hits" => Phrase::Hits,
            "single" => Phrase::Single,
            "groove" => Phrase::Groove,
            _ => Phrase::Melody,
        }
    }
}

/// One instrument of the palette.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Stable id: `gm:<bank>:<program>`, `gmkit:<program>`.
    pub id: String,
    pub name: String,
    pub kind: Kind,
    /// keys mallet organ guitar bass strings ensemble choir brass reed pipe lead pad fx world perc drums.
    pub family: String,
    /// bass chords lead pad arp texture hits fx drums.
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    /// The MIDI note it is auditioned and measured at.
    pub root: u8,
    pub phrase: Phrase,
    /// Measured (none until `score_palette measure-gm` ran).
    pub features: Option<Features>,
    /// Words the measurement earns, relative to its family.
    pub measured: Vec<String>,
}

impl Entry {
    /// The Score instrument node that plays it (the part after `let x = `).
    pub fn code(&self) -> String {
        match self.kind {
            Kind::Gm { bank: 0, program } => format!("Instrument{{kind: @soundfont program: {program}}}"),
            Kind::Gm { bank, program } => format!("Instrument{{kind: @soundfont bank: {bank} program: {program}}}"),
            Kind::GmKit { program } => format!("Instrument{{kind: @soundfont bank: 128 program: {program}}}"),
            Kind::Kit { name } => format!("Drums{{kit: @{name}}}"),
            Kind::Patch { name } => format!("Synth{{preset: @{name}}}"),
        }
    }

    /// A one-line description: name, family, roles, tags and measured words.
    pub fn about(&self) -> String {
        let mut words: Vec<&str> = self.tags.iter().map(String::as_str).collect();
        words.extend(self.measured.iter().map(String::as_str));
        format!("{} ({}; {}): {}", self.name, self.family, self.roles.join("/"), words.join(" ").replace('_', " "))
    }
}

fn rows(text: &str) -> impl Iterator<Item = Vec<&str>> {
    text.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).map(|l| l.split('\t').collect())
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_string).collect()
}

/// Every instrument the palette knows, with its measurements.
pub fn entries() -> Vec<Entry> {
    let mut out = Vec::new();
    for r in rows(GM) {
        if r.len() < 8 {
            continue;
        }
        let (bank, program) = (r[0].parse().unwrap_or(0), r[1].parse().unwrap_or(0));
        out.push(Entry {
            id: format!("gm:{bank}:{program}"),
            name: r[2].to_string(),
            kind: Kind::Gm { bank, program },
            family: r[3].to_string(),
            roles: words(r[4]),
            root: r[5].parse().unwrap_or(60),
            phrase: Phrase::parse(r[6]),
            tags: words(r[7]),
            features: None,
            measured: Vec::new(),
        });
    }
    for r in rows(GM_KITS) {
        if r.len() < 4 {
            continue;
        }
        let program = r[0].parse().unwrap_or(0);
        out.push(Entry {
            id: format!("gmkit:{program}"),
            name: format!("{} kit", r[1]),
            kind: Kind::GmKit { program },
            family: "drums".into(),
            roles: vec!["drums".into()],
            root: 36,
            phrase: Phrase::Groove,
            tags: {
                let mut t = words(r[3]);
                t.extend(words(&r[2].replace([',', ':', '(', ')'], " ")));
                t.sort();
                t.dedup();
                t
            },
            features: None,
            measured: Vec::new(),
        });
    }
    let lib = sounds::sounds();
    for k in &lib.kits {
        let mut tags: Vec<String> = k.tags.clone();
        tags.extend(words(&k.about.replace([',', ':', '(', ')', '-'], " ").to_ascii_lowercase()));
        tags.push("synthesized".into());
        tags.sort();
        tags.dedup();
        out.push(Entry {
            id: format!("kit:{}", k.name),
            name: format!("{} kit", k.name.replace('_', " ")),
            kind: Kind::Kit { name: k.name.as_str() },
            family: "drums".into(),
            roles: vec!["drums".into()],
            root: 36,
            phrase: Phrase::Groove,
            tags,
            features: None,
            measured: Vec::new(),
        });
    }
    for p in &lib.patches {
        let (root, phrase) = match p.family.as_str() {
            "bass" => (36, Phrase::Bass),
            "lead" | "bell" => (72, Phrase::Melody),
            "pluck" => (60, Phrase::Melody),
            "fx" => (60, Phrase::Single),
            _ => (60, Phrase::Chords),
        };
        let mut tags = p.tags.clone();
        tags.push("synth".into());
        tags.push("synthesized".into());
        out.push(Entry {
            id: format!("patch:{}", p.name),
            name: p.name.replace('_', " "),
            kind: Kind::Patch { name: p.name.as_str() },
            family: p.family.clone(),
            roles: p.roles.clone(),
            root,
            phrase,
            tags,
            features: None,
            measured: Vec::new(),
        });
    }
    for r in rows(CATALOGUE) {
        let Some(entry) = out.iter_mut().find(|e| e.id == r[0]) else { continue };
        let values: Vec<f32> = r.iter().skip(1).take(Features::COLUMNS.len()).filter_map(|v| v.parse().ok()).collect();
        entry.features = Features::from_values(&values);
        if let Some(measured) = r.get(1 + Features::COLUMNS.len()) {
            entry.measured = words(measured);
        }
    }
    out
}

/// The words a measurement earns, against the other entries of its
/// family (`peers`): the brightest third is `bright`, the darkest `dark`, and so on.
/// A kit (`kit`) is measured as a groove, so only its ranks speak.
pub fn describe(f: &Features, peers: &[Features], kit: bool) -> Vec<String> {
    let rank = |value: f32, key: fn(&Features) -> f32| -> f32 {
        if peers.len() < 3 {
            return 0.5;
        }
        peers.iter().filter(|p| key(p) < value).count() as f32 / (peers.len() - 1) as f32
    };
    let mut out = Vec::new();
    let mut tercile = |r: f32, lo: &str, hi: &str| {
        if r <= 0.3 {
            out.push(lo.to_string());
        } else if r >= 0.7 {
            out.push(hi.to_string());
        }
    };
    tercile(rank(f.centroid, |p| p.centroid), "dark", "bright");
    tercile(rank(f.low, |p| p.low), "thin", "full");
    tercile(rank(f.decay_ms, |p| p.decay_ms), "short", "long");
    tercile(rank(f.lufs, |p| p.lufs), "quiet", "loud");
    if kit {
        return out;
    }
    // Absolute words.
    if f.attack_ms <= 8.0 {
        out.push("punchy".into());
    } else if f.attack_ms >= 120.0 {
        out.push("slow_attack".into());
    }
    if f.centroid > 3500.0 {
        out.push("airy".into());
    }
    if f.low > 0.6 {
        out.push("boomy".into());
    }
    if f.harsh_db > 4.0 {
        out.push("harsh".into());
    } else if f.harsh_db > -4.0 {
        out.push("present".into());
    }
    out
}

/// The gain that brings an entry to its family's median measured
/// loudness (0.25..4; none unmeasured): what to put on `gain:` so a part
/// sits where its family usually sits.
pub fn balance_gain(entry: &Entry) -> Option<f32> {
    let mine = entry.features?.lufs;
    let mut levels: Vec<f32> = entries().iter().filter(|e| e.family == entry.family).filter_map(|e| e.features.map(|f| f.lufs)).collect();
    if levels.len() < 3 {
        return None;
    }
    levels.sort_by(f32::total_cmp);
    let median = levels[levels.len() / 2];
    Some(10.0_f32.powf((median - mine) / 20.0).clamp(0.25, 4.0))
}

/// Query words that mean a tag (or a measured word) by another name.
const SYNONYMS: &[(&str, &str)] = &[
    ("drum", "drums"), ("kit", "drums"), ("beat", "drums"), ("percussion", "perc"),
    ("machine", "drum_machine"), ("eighties", "80s"), ("seventies", "70s"), ("nineties", "90s"),
    ("sixties", "60s"), ("warm", "warm"), ("mellow", "soft"), ("gentle", "soft"), ("smooth", "soft"), ("hard", "punchy"), ("tight", "short"),
    ("snappy", "punchy"), ("fat", "full"), ("deep", "full"), ("heavy", "full"), ("thick", "full"), ("sub", "boomy"), ("shrill", "bright"),
    ("crisp", "bright"), ("sparkly", "sparkle"), ("dull", "dark"), ("muffled", "dark"), ("sustained", "long"), ("plucky", "plucked"),
    ("pluck", "plucked"), ("keys", "keys"), ("rhodes", "electric_piano"), ("ep", "electric_piano"), ("epiano", "electric_piano"),
    ("upright", "upright"), ("double", "double_bass"), ("orchestra", "orchestral"), ("cinematic", "film"), ("movie", "film"),
    ("vocal", "vocal"), ("voice", "vocal"), ("choral", "choir"), ("synthesizer", "synth"), ("electric", "electric"),
    ("acoustic", "acoustic"), ("big", "big"), ("huge", "big"), ("roomy", "room"), ("live", "live"), ("gated", "gated"),
    ("lofi", "lofi"), ("lo-fi", "lofi"), ("hiphop", "hip_hop"), ("hip-hop", "hip_hop"), ("rnb", "soul"), ("r&b", "soul"),
    ("edm", "edm"), ("dance", "house"), ("disco", "disco"), ("brushes", "brush"), ("strings", "strings"), ("string", "strings"),
    ("horns", "brass"), ("horn", "brass"), ("sax", "sax"), ("saxophone", "sax"), ("guitars", "guitar"), ("basses", "bass"),
    ("pads", "pad"), ("leads", "lead"), ("bells", "bell"), ("mallets", "mallet"), ("organ", "organ"), ("piano", "piano"),
];

fn normal(word: &str) -> String {
    let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '&' && c != '-').to_ascii_lowercase();
    let w = w.replace('-', "_");
    SYNONYMS.iter().find(|(k, _)| *k == w).map(|(_, v)| v.to_string()).unwrap_or(w)
}

/// One answer of a search.
#[derive(Clone, Debug)]
pub struct Hit {
    pub entry: Entry,
    pub score: f32,
    /// The query words it matched.
    pub matched: Vec<String>,
}

/// The instruments that fit a description, best first. `kind` narrows to
/// `drums` (kits) or `melodic`; `role` to bass/chords/lead/pad/arp/...
pub fn search(query: &str, kind: Option<&str>, role: Option<&str>, limit: usize) -> Vec<Hit> {
    let terms: Vec<String> = query.split_whitespace().map(normal).filter(|w| !w.is_empty() && !matches!(w.as_str(), "a" | "an" | "the" | "and" | "with" | "for" | "of" | "sound" | "sounding")).collect();
    let mut hits: Vec<Hit> = entries()
        .into_iter()
        .filter(|e| match kind {
            Some("drums") | Some("kit") | Some("kits") => e.kind.is_kit(),
            Some("melodic") | Some("instrument") => !e.kind.is_kit(),
            _ => true,
        })
        .filter(|e| role.is_none_or(|r| e.roles.iter().any(|x| x == r) || (r == "drums" && e.family == "drums")))
        .filter_map(|e| {
            let name = e.name.to_ascii_lowercase();
            let mut score = 0.0;
            let mut matched = Vec::new();
            for t in &terms {
                let s = if e.family == *t || e.roles.iter().any(|r| r == t) {
                    3.0
                } else if e.tags.iter().any(|x| x == t) {
                    2.0
                } else if e.measured.iter().any(|x| x == t) {
                    1.5
                } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w == t.as_str()) {
                    2.0
                } else if t.len() >= 4 && e.tags.iter().chain(&e.measured).any(|x| x.split('_').any(|w| w == t.as_str()) || x.starts_with(t.as_str())) {
                    1.0
                } else {
                    0.0
                };
                if s > 0.0 {
                    score += s;
                    matched.push(t.clone());
                }
            }
            // The plain variations rank below the kit or program they vary.
            if e.name.chars().last().is_some_and(|c| c.is_ascii_digit()) && matches!(e.kind, Kind::GmKit { .. }) {
                score -= 0.5;
            }
            (score > 0.0 || terms.is_empty()).then_some(Hit { entry: e, score, matched })
        })
        .collect();
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.entry.id.cmp(&b.entry.id)));
    hits.truncate(limit.max(1));
    hits
}

/// A note list for an audition of `entry` (beats, note, dur, vel), on a 100 bpm clock.
pub fn audition_notes(entry: &Entry) -> Vec<(f32, u8, f32, f32)> {
    let r = entry.root;
    let n = |d: i32| (r as i32 + d).clamp(0, 127) as u8;
    match entry.phrase {
        Phrase::Melody => {
            let mut v = vec![(0.0, n(0), 0.5, 0.85), (0.5, n(2), 0.5, 0.7), (1.0, n(4), 0.5, 0.8), (1.5, n(7), 0.5, 0.7), (2.0, n(9), 1.0, 0.95), (3.0, n(7), 0.5, 0.7), (3.5, n(4), 0.5, 0.65)];
            v.extend([(4.0, n(0), 3.0, 0.75), (4.0, n(4), 3.0, 0.7), (4.0, n(7), 3.0, 0.7), (4.0, n(11), 3.0, 0.7)]);
            v
        }
        Phrase::Chords => {
            let mut v = Vec::new();
            for (beat, chord, vel) in [(0.0, [0, 4, 7, 11], 0.75), (2.0, [-3, 0, 4, 7], 0.7), (4.0, [-7, -3, 0, 5], 0.75), (6.0, [-5, -1, 2, 7], 0.8)] {
                for d in chord {
                    v.push((beat, n(d), 1.9, vel));
                }
            }
            v
        }
        Phrase::Bass => {
            let line = [(0.0, 0, 0.45, 0.9), (0.5, 0, 0.45, 0.6), (1.0, 12, 0.45, 0.75), (1.5, 0, 0.45, 0.6), (2.0, 7, 0.95, 0.85), (3.0, 5, 0.45, 0.7), (3.5, 7, 0.45, 0.65)];
            let mut v: Vec<_> = line.iter().map(|(b, d, du, ve)| (*b, n(*d), *du, *ve)).collect();
            v.extend(line.iter().map(|(b, d, du, ve)| (*b + 4.0, n(*d - 4), *du, *ve)));
            v.push((8.0, n(0), 2.0, 0.85));
            v
        }
        Phrase::Hits => [0.0, 0.75, 1.5, 2.0, 3.0, 3.5, 4.0, 4.5, 5.0, 6.0].iter().enumerate().map(|(i, b)| (*b, n([0, 0, 7, 0, 5, 7, 0, 3, 5, 0][i]), 0.4, if i % 2 == 0 { 0.9 } else { 0.65 })).collect(),
        Phrase::Single => vec![(0.0, n(0), 4.0, 0.85)],
        Phrase::Groove => groove(),
    }
}

/// Two bars of groove with a tom fill, then a bar on the ride with the
/// kit's other pieces, at 100 bpm (General MIDI drum notes).
fn groove() -> Vec<(f32, u8, f32, f32)> {
    let mut v = Vec::new();
    for bar in 0..2 {
        let b = bar as f32 * 4.0;
        for (beat, note, vel) in [(0.0, 36, 0.95), (1.5, 36, 0.7), (2.5, 36, 0.85), (1.0, 38, 0.95), (3.0, 38, 0.95)] {
            if bar == 1 && beat >= 2.0 {
                continue;
            }
            v.push((b + beat, note, 0.25, vel));
        }
        for i in 0..8 {
            if bar == 1 && i >= 4 {
                break;
            }
            let (note, vel) = if bar == 0 && i == 7 { (46, 0.8) } else { (42, if i % 2 == 0 { 0.8 } else { 0.55 }) };
            v.push((b + i as f32 * 0.5, note, 0.25, vel));
        }
        if bar == 0 {
            v.push((2.75, 38, 0.25, 0.35));
        }
    }
    for (i, note) in [50, 50, 48, 48, 47, 45, 43, 43].iter().enumerate() {
        v.push((6.0 + i as f32 * 0.25, *note, 0.25, 0.75 + 0.03 * i as f32));
    }
    v.push((8.0, 36, 0.25, 1.0));
    v.push((8.0, 49, 0.25, 0.95));
    for (beat, note, vel) in [(9.0, 51, 0.75), (9.5, 51, 0.6), (10.0, 53, 0.8), (10.5, 51, 0.6), (11.0, 51, 0.75), (11.5, 51, 0.6)] {
        v.push((beat, note, 0.25, vel));
    }
    for (beat, note, vel) in [(9.0, 37, 0.8), (10.0, 39, 0.85), (10.5, 54, 0.7), (11.0, 56, 0.75), (11.25, 44, 0.7), (11.5, 75, 0.75), (11.75, 70, 0.7), (10.0, 36, 0.85)] {
        v.push((beat, note, 0.25, vel));
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

/// A whole Score that auditions `entry` (dry but for a short room).
pub fn audition_score(entry: &Entry) -> String {
    let notes: Vec<String> = audition_notes(entry).iter().map(|(b, n, d, v)| format!("[{b} {n} {d} {v}]")).collect();
    format!(
        "use mod.score.*\nlet clock = Clock{{bpm: 100}}\nlet inst = {}\nlet part = Loop{{play: inst beats: 32 notes: [{}]}}\nlet room = Reverb{{in: [part] room: 0.35 mix: 0.12}}\nScore{{clock inst part room}}\n",
        entry.code(),
        notes.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gm_tables_read_whole() {
        let all = entries();
        assert_eq!(all.iter().filter(|e| matches!(e.kind, Kind::Gm { bank: 0, .. })).count(), 128);
        assert!(all.iter().filter(|e| matches!(e.kind, Kind::GmKit { .. })).count() >= 8);
        assert!(all.iter().filter(|e| matches!(e.kind, Kind::Kit { .. })).count() >= 15);
        assert!(all.iter().filter(|e| matches!(e.kind, Kind::Patch { .. })).count() >= 45);
        for e in &all {
            assert!(!e.tags.is_empty() && !e.roles.is_empty(), "{}", e.id);
            let source = audition_score(e);
            crate::model::ScoreDoc::load(&source).unwrap_or_else(|d| panic!("{}: {d:?}\n{source}", e.id));
        }
    }

    #[test]
    fn a_description_finds_what_it_describes() {
        let first = |q: &str, kind: Option<&str>| search(q, kind, None, 3).into_iter().map(|h| h.entry.id).collect::<Vec<_>>();
        assert!(first("80s analog drum machine", None).contains(&"gmkit:25".to_string()), "{:?}", first("80s analog drum machine", None));
        assert!(first("brush jazz kit", Some("drums")).contains(&"gmkit:40".to_string()));
        assert!(first("upright jazz bass", None).contains(&"gm:0:32".to_string()));
        assert!(first("fretless bass", None).contains(&"gm:0:35".to_string()));
        assert_eq!(first("big gated 80s rock drums", None)[0], "gmkit:16");
    }
}

#[cfg(test)]
mod tool_tests {
    #[test]
    fn instrument_search_answers_with_code_and_measures() {
        let args = makepad_strict_json::parse(br#"{"query": "warm upright jazz bass", "limit": 3}"#).unwrap();
        let out = crate::tools::call("instrument_search", &args, None).unwrap();
        let results = match out.get("results") {
            Some(makepad_strict_json::Value::Arr(r)) => r.clone(),
            other => panic!("{other:?}"),
        };
        assert!(!results.is_empty());
        let code = results[0].get("code").and_then(makepad_strict_json::Value::as_str).unwrap();
        assert!(code.starts_with("Instrument{kind: @soundfont"), "{code}");
        assert!(results[0].get("measured").is_some());
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    /// Every library kit and patch is measured (`score_palette measure`
    /// after a change) and passes the gate: no clipping, no DC, no
    /// non-finite samples, no harsh 2-5 kHz spike, a sane loudness.
    #[test]
    fn the_library_is_measured_and_passes_the_gate() {
        for e in entries().iter().filter(|e| matches!(e.kind, Kind::Kit { .. } | Kind::Patch { .. })) {
            let f = e.features.unwrap_or_else(|| panic!("{} is not measured: run score_palette measure", e.id));
            assert!(f.faults().is_empty(), "{}: {:?}", e.id, f.faults());
        }
    }
}
