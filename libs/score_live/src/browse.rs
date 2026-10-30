//! The preset index the instrument browser and the AI search: every
//! instrument a score can become ([`crate::instruments::catalog`]) as an
//! entry with its engine, category, character tags, pack, a short line, a
//! small picture and a cheap feature vector ("similar to this").
//!
//! Built once, off the UI thread ([`build`], kept by [`set_index`]): the
//! engines' built-in libraries (synth patches, kits, Morph, General MIDI,
//! the piano, Ironfish, shader instruments, voices). What a score adds
//! (its own instruments, its SoundFonts and samples) is a handful of
//! entries made when the browser opens ([`score_entries`]). Search is a
//! scan of prepared lowercase text: thousands of entries in well under a
//! millisecond, per keystroke.
//!
//! A new engine appears by adding its presets to the catalog with its type
//! name: its engine facet, categories (from its words) and picture follow.

use crate::instruments::{Available, Choice};
use crate::model::ScoreDoc;
use std::sync::OnceLock;

/// The categories, in the order a browser lists them.
pub const CATEGORIES: [&str; 20] = [
    "bass", "lead", "pad", "keys", "pluck", "bell", "arp", "strings", "brass", "woodwind", "guitar", "organ", "vocal", "drums", "perc", "fx", "drone",
    "world", "sample", "other",
];

/// A preset's small picture.
#[derive(Clone, Debug, PartialEq)]
pub enum Thumb {
    /// An envelope (attack, decay, sustain, release) over a wave (0 sine,
    /// 1 triangle, 2 saw, 3 square, 4 pulse, 5 noise, 6 FM, 7 string).
    Env { adsr: [f32; 4], wave: u8 },
    /// A wavetable's frame: partial amplitudes.
    Table { partials: [f32; 8] },
    /// A kit: which of its sixteen voices sound (a bit each).
    Kit { slots: u16 },
    /// Its category's mark.
    Mark,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub choice: Choice,
    /// Its engine (`Synth`, `Morph`, `Drums`, `GM`, `Piano` …).
    pub engine: &'static str,
    pub category: &'static str,
    /// Character tags (`bright`, `dark`, `mono`, `80s`, `acid` …).
    pub tags: Vec<String>,
    /// Where it comes from (`Makepad`, `General MIDI`, a SoundFont, a score).
    pub pack: String,
    pub about: String,
    pub thumb: Thumb,
    /// Brightness, attack, length, motion, width, weight (0..1 each): what
    /// "similar" compares.
    pub features: [f32; 6],
    /// Name, tags, pack, category, engine: lowercase, for the search.
    hay: String,
}

impl Entry {
    fn new(choice: Choice, engine: &'static str, category: &'static str, tags: Vec<String>, pack: &str, about: String, thumb: Thumb, features: [f32; 6]) -> Entry {
        let hay = format!("{} {} {} {} {} {}", choice.label, tags.join(" "), pack, category, engine, about).to_lowercase().replace('_', " ");
        Entry { choice, engine, category, tags, pack: pack.to_string(), about, thumb, features, hay }
    }

    /// Whether every word of `query` is in its name, tags, pack, category,
    /// engine or line.
    pub fn matches(&self, words: &[String]) -> bool {
        words.iter().all(|w| self.hay.contains(w.as_str()))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Index {
    pub entries: Vec<Entry>,
}

/// What a browser filters by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facets {
    pub engine: Option<String>,
    pub category: Option<String>,
    pub pack: Option<String>,
    /// Every tag must be there.
    pub tags: Vec<String>,
}

impl Index {
    /// The entries matching `query` (words) and `facets`, in index order.
    pub fn search(&self, query: &str, facets: &Facets) -> Vec<usize> {
        search_in(&self.entries, query, facets)
    }

    /// The `n` entries most like entry `of` (by features, same engine
    /// family first), nearest first.
    pub fn similar(&self, of: usize, n: usize) -> Vec<usize> {
        similar_in(&self.entries, of, n)
    }
}

/// The search over any list of entries (the index and a score's own).
pub fn search_in(entries: &[Entry], query: &str, facets: &Facets) -> Vec<usize> {
    let words: Vec<String> = query.to_lowercase().replace('_', " ").split_whitespace().map(str::to_string).collect();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            facets.engine.as_deref().is_none_or(|x| e.engine == x)
                && facets.category.as_deref().is_none_or(|x| e.category == x)
                && facets.pack.as_deref().is_none_or(|x| e.pack == x)
                && facets.tags.iter().all(|t| e.tags.iter().any(|x| x == t))
                && e.matches(&words)
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn similar_in(entries: &[Entry], of: usize, n: usize) -> Vec<usize> {
    let Some(me) = entries.get(of) else { return Vec::new() };
    let mut d: Vec<(f32, usize)> = entries
        .iter()
        .enumerate()
        .filter(|(i, e)| *i != of && e.category != "other" && !matches!(e.engine, "MIDI" | "Input"))
        .map(|(i, e)| {
            let f: f32 = me.features.iter().zip(e.features.iter()).map(|(a, b)| (a - b) * (a - b)).sum();
            let cat = if e.category == me.category { 0.0 } else { 0.35 };
            (f + cat, i)
        })
        .collect();
    d.sort_by(|a, b| a.0.total_cmp(&b.0));
    d.into_iter().take(n).map(|(_, i)| i).collect()
}

/// A preset library an engine offers as an opt-in download (its licence
/// shown before anything is installed): the browser offers it under the
/// engine until it is installed, then its presets join the index.
#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub engine: String,
    pub title: String,
    /// The licence by name.
    pub licence: String,
    pub installed: bool,
}

/// The downloadable libraries of the engines (none of the built-in engines
/// has one; an engine with a library adds it here).
pub fn libraries() -> Vec<Library> {
    Vec::new()
}

static INDEX: OnceLock<Index> = OnceLock::new();

/// The index, once built.
pub fn index() -> Option<&'static Index> {
    INDEX.get()
}

/// Keep the index [`build`] made (the first one wins).
pub fn set_index(index: Index) {
    let _ = INDEX.set(index);
}

/// The index of the built-in libraries, built where it is called (a
/// background job: it evaluates the sound library once).
pub fn build() -> Index {
    let all = crate::instruments::catalog(None, &Available::default());
    Index { entries: all.into_iter().map(entry_of).collect() }
}

/// The index for tools and tests that cannot wait for a background build.
pub fn index_or_build() -> &'static Index {
    INDEX.get_or_init(build)
}

/// What a score adds to the index: its own instruments and the SoundFonts,
/// samples and singers it names.
pub fn score_entries(doc: Option<&ScoreDoc>, available: &Available) -> Vec<Entry> {
    let built = crate::instruments::catalog(None, &Available::default()).into_iter().map(|c| c.key()).collect::<std::collections::HashSet<_>>();
    crate::instruments::catalog(doc, available).into_iter().filter(|c| !built.contains(&c.key())).map(entry_of).collect()
}

/// A category from words (a description, tags, a name).
fn category_of(text: &str) -> &'static str {
    let t = text.to_lowercase();
    let has = |w: &str| t.split(|c: char| !c.is_alphanumeric()).any(|x| x == w || (w.len() > 3 && x.starts_with(w)));
    for (cat, words) in [
        ("drums", &["drum", "kit", "kick", "snare"][..]),
        ("bass", &["bass", "sub", "reese", "808"][..]),
        ("pad", &["pad", "cloud", "ambient", "atmosphere", "choir"][..]),
        ("lead", &["lead", "solo", "supersaw", "sync"][..]),
        ("pluck", &["pluck", "pizzicato", "harp"][..]),
        ("bell", &["bell", "glass", "mallet", "chime", "glockenspiel", "vibraphone", "marimba"][..]),
        ("arp", &["arp", "arpeggio", "sequence"][..]),
        ("keys", &["piano", "keys", "epiano", "rhodes", "clav", "harpsichord"][..]),
        ("organ", &["organ"][..]),
        ("strings", &["string", "strings", "violin", "cello"][..]),
        ("brass", &["brass", "trumpet", "horn"][..]),
        ("drone", &["drone", "chaos"][..]),
        ("fx", &["fx", "noise", "riser", "zap", "laser", "effect"][..]),
        ("vocal", &["vocal", "voice", "vowel", "formant", "sing", "sung"][..]),
    ] {
        if words.iter().any(|w| has(w)) {
            return cat;
        }
    }
    "other"
}

/// A General MIDI family's category.
fn gm_category(family: usize) -> &'static str {
    ["keys", "bell", "organ", "guitar", "bass", "strings", "strings", "brass", "woodwind", "woodwind", "lead", "pad", "fx", "world", "perc", "fx"][family.min(15)]
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '_').to_lowercase()).filter(|w| w.len() > 1).collect()
}

fn entry_of(choice: Choice) -> Entry {
    use makepad_audio_synth::dsp::{FilterMode, Wave};
    use makepad_audio_synth::recipe::Source;
    let field = |k: &str| choice.fields.iter().find(|(key, _)| key == k).map(|(_, v)| v.trim_start_matches('@').trim_matches('"').to_string());
    let sounds = crate::palette::sounds::sounds();
    let flat = |c: &str| -> [f32; 6] {
        let i = CATEGORIES.iter().position(|x| *x == c).unwrap_or(0) as f32 / CATEGORIES.len() as f32;
        [0.5, 0.5, 0.5, 0.3, 0.5, i]
    };
    match (choice.type_name, choice.group) {
        (_, "My instruments") => {
            let about = choice.detail.clone();
            let cat = category_of(&format!("{} {}", choice.label, about));
            Entry::new(choice, "Mine", cat, Vec::new(), "My scores", about, Thumb::Mark, flat(cat))
        }
        ("Synth", _) => {
            let name = field("preset").unwrap_or_default();
            let patch = sounds.patch(&name);
            let (family, mut tags, about, layer, mono) = match patch {
                Some(p) => (p.family.clone(), p.tags.clone(), p.about.clone(), p.recipe.layers().first().copied().unwrap_or_default(), p.mono),
                None => (String::new(), Vec::new(), choice.detail.clone(), Default::default(), false),
            };
            let cat = CATEGORIES.iter().copied().find(|c| family.trim_start_matches('@') == *c || (*c == "vocal" && family.contains("choir"))).unwrap_or_else(|| category_of(&about));
            let wave = match layer.source {
                Source::Osc(Wave::Sine) => 0,
                Source::Osc(Wave::Triangle) => 1,
                Source::Osc(Wave::Square) => 3,
                Source::Osc(Wave::Pulse) => 4,
                Source::Noise(_) => 5,
                Source::Fm { .. } => 6,
                Source::Pluck { .. } => 7,
                _ => 2,
            };
            let bright = if layer.filter == FilterMode::Off { 0.9 } else { ((layer.cutoff.max(30.0) / 30.0).ln() / (16000.0f32 / 30.0).ln()).clamp(0.0, 1.0) };
            tags.push(if bright > 0.6 { "bright" } else if bright < 0.35 { "dark" } else { "warm" }.to_string());
            tags.push(if mono { "mono" } else { "poly" }.to_string());
            let attack = (layer.attack / 0.5).clamp(0.0, 1.0);
            let length = ((layer.decay + layer.release) / 4.0).clamp(0.0, 1.0) * (0.3 + 0.7 * layer.sustain);
            let motion = (layer.vib_depth * 2.0 + if layer.cutoff_to.is_finite() { 0.5 } else { 0.0 }).clamp(0.0, 1.0);
            let features = [bright, attack, length, motion, (layer.detune_cents.abs() / 30.0).clamp(0.0, 1.0), if cat == "bass" { 1.0 } else { 0.3 }];
            Entry::new(choice, "Synth", cat, tags, "Makepad", about, Thumb::Env { adsr: [layer.attack, layer.decay, layer.sustain, layer.release], wave }, features)
        }
        ("Drums", _) => {
            let name = field("kit").unwrap_or_default();
            let kit = sounds.kit(&name);
            let slots = kit.map_or(0, |k| k.slots.iter().enumerate().filter(|(_, s)| s.is_some()).fold(0u16, |m, (i, _)| m | (1 << i.min(15))));
            let tags = kit.map(|k| k.tags.clone()).unwrap_or_default();
            let about = kit.map_or_else(|| choice.detail.clone(), |k| k.about.clone());
            let features = [if tags.iter().any(|t| t.contains("lofi") || t.contains("dusty")) { 0.3 } else { 0.6 }, 0.0, 0.3, 0.1, 0.5, 0.6];
            Entry::new(choice, "Drums", "drums", tags, "Makepad", about, Thumb::Kit { slots }, features)
        }
        ("Morph", _) => {
            let about = choice.detail.trim_start_matches("Morph wavetable synth: ").to_string();
            let cat = category_of(&format!("{} {}", choice.label, about));
            let mut tags = words(&about);
            tags.retain(|t| t.len() > 3);
            tags.truncate(6);
            // The table's colour: a few partials from its words.
            let bright = if about.contains("bright") || about.contains("saw") { 0.8 } else if about.contains("dark") || about.contains("sub") { 0.25 } else { 0.5 };
            let mut partials = [0f32; 8];
            for (h, p) in partials.iter_mut().enumerate() {
                *p = (1.0 / (h as f32 + 1.0)).powf(1.6 - bright);
            }
            let features = [bright, if cat == "pad" { 0.7 } else { 0.1 }, if cat == "pad" || cat == "drone" { 0.9 } else { 0.4 }, 0.6, 0.6, if cat == "bass" { 1.0 } else { 0.4 }];
            Entry::new(choice, "Morph", cat, tags, "Makepad", about, Thumb::Table { partials }, features)
        }
        ("Instrument", group) => {
            let kind = field("kind").unwrap_or_default();
            match kind.as_str() {
                "drums" => Entry::new(choice.clone(), "Drums", "drums", vec!["acoustic".into(), "modelled".into()], "Makepad", choice.detail.clone(), Thumb::Kit { slots: 0xffff }, [0.6, 0.0, 0.4, 0.1, 0.5, 0.6]),
                "piano" => Entry::new(choice.clone(), "Piano", "keys", vec!["acoustic".into(), "modelled".into()], "Makepad", choice.detail.clone(), Thumb::Env { adsr: [0.002, 2.5, 0.0, 0.6], wave: 0 }, [0.6, 0.0, 0.8, 0.1, 0.6, 0.5]),
                "midi" => Entry::new(choice.clone(), "MIDI", "other", vec!["outboard".into()], "MIDI", choice.detail.clone(), Thumb::Mark, [0.0; 6]),
                "input" => Entry::new(choice.clone(), "Input", "other", vec!["live".into()], "Audio", choice.detail.clone(), Thumb::Mark, [0.0; 6]),
                _ => {
                    let bank = field("bank").unwrap_or_default();
                    let program: usize = field("program").and_then(|p| p.parse().ok()).unwrap_or(0);
                    let font = if group == "SoundFonts" { choice.label.split(" · ").next().unwrap_or("SoundFont").to_string() } else { "General MIDI".to_string() };
                    if bank == "128" {
                        Entry::new(choice.clone(), "GM", "drums", vec!["general_midi".into()], &font, choice.detail.clone(), Thumb::Kit { slots: 0xffff }, [0.5, 0.0, 0.4, 0.1, 0.5, 0.6])
                    } else {
                        let cat = gm_category(program / 8);
                        let f = flat(cat);
                        Entry::new(choice.clone(), "GM", cat, vec![crate::instruments::GM_FAMILIES[(program / 8).min(15)].trim_start_matches("GM ").to_string()], &font, choice.detail.clone(), Thumb::Mark, f)
                    }
                }
            }
        }
        ("Ironfish", _) => {
            let cat = match choice.label.as_str() {
                "acid" | "sub" => "bass",
                "pad" | "glass" => "pad",
                "supersaw" | "chip" => "lead",
                "formant" => "vocal",
                _ => "other",
            };
            let f = flat(cat);
            Entry::new(choice.clone(), "Ironfish", cat, Vec::new(), "Makepad", choice.detail.clone(), Thumb::Mark, f)
        }
        ("Shader", _) => {
            let cat = category_of(&format!("{} {}", choice.label, choice.detail));
            let f = flat(cat);
            Entry::new(choice.clone(), "Shader", cat, words(&choice.detail).into_iter().filter(|w| w.len() > 3).take(5).collect(), "Makepad", choice.detail.clone(), Thumb::Mark, f)
        }
        ("Voice", group) => {
            let pack = if group == "Library singers" { "Library" } else { "Makepad" };
            Entry::new(choice.clone(), "Voice", "vocal", Vec::new(), pack, choice.detail.clone(), Thumb::Mark, flat("vocal"))
        }
        ("Sampler", _) => Entry::new(choice.clone(), "Sampler", "sample", Vec::new(), "Library", choice.detail.clone(), Thumb::Mark, flat("sample")),
        (ty, _) => {
            // An engine this file has not met: its type is its engine.
            let engine: &'static str = crate::instruments::INSTRUMENT_TYPES.iter().copied().find(|t| *t == ty).unwrap_or("Other");
            let cat = category_of(&format!("{} {}", choice.label, choice.detail));
            let f = flat(cat);
            Entry::new(choice.clone(), engine, cat, Vec::new(), "Makepad", choice.detail.clone(), Thumb::Mark, f)
        }
    }
}

/// The index as the AI reads it: the entries matching words (and an
/// engine or category), each as a line it can write.
pub fn describe(entries: &[(usize, &Entry)], limit: usize) -> Vec<String> {
    entries
        .iter()
        .take(limit)
        .map(|(_, e)| {
            let fields: Vec<String> = e.choice.fields.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            format!("{}{{{}}}  [{} · {} · {}] {}  {}", e.choice.type_name, fields.join(" "), e.engine, e.category, e.pack, e.choice.label, e.about.chars().take(90).collect::<String>())
        })
        .collect()
}
