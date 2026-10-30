//! The Code view's help while typing: completions for what can be written
//! where the cursor is (node types, a node's fields with their ranges, the
//! atoms a field takes, the instruments and loops of this score, the loop
//! code's functions), docs for the name under the mouse, and "did you
//! mean" for a near miss. Everything is read from the live registries (the
//! node types the VM knows, the model's field lists, the presets, kits,
//! shader library, piano presets, GM programs, the library's SoundFonts,
//! samples and singers) and the score itself. Only the text before the
//! cursor is looked at: cheap enough for every keystroke.

use crate::instruments::{Available, GM_PROGRAMS, INSTRUMENT_TYPES};
use crate::source::{self, Tok};
use makepad_score_player::presets;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    NodeType,
    Field,
    Atom,
    Node,
    Function,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub label: String,
    /// What replaces the word being typed.
    pub insert: String,
    pub detail: String,
    pub kind: ItemKind,
}

/// Completions for the word from `start` to the cursor.
#[derive(Clone, Debug, PartialEq)]
pub struct Completions {
    pub start: usize,
    pub end: usize,
    pub items: Vec<Item>,
}

/// Most items offered at once.
const MAX_ITEMS: usize = 60;

/// Each node type in one line.
pub fn type_doc(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "Clock" => "tempo, signature, swing, seed and the game params (bpm sig swing seed params tempo)",
        "Synth" => "a recipe synth: a preset or oscillators, filter and envelope, overridable per note",
        "Drums" => "a synthesized drum kit (analog_80s analog_hybrid studio lofi chip), sixteen slots",
        "Sampler" => "a library sample played pitched from its root note",
        "Ironfish" => "Ironfish: a rich 16-voice synth with its own chorus, delay and reverb",
        "Morph" => "Morph: a wavetable synth (3 wavetable/granular/spectral/sample oscs with warps and unison, sub, noise, 2 filters, 4 envs, 8 LFOs, 8 macros, mod rows, arp, fx rack); morph_params lists its parameters",
        "Tandem" => "Tandem: the hybrid two-scene synth that plays the hybrid patch library (patch: \"Category/Name\"); tandem_patches finds patches, tandem_params lists its parameters",
        "Shader" => "an instrument written as an audio shader: a library one or your own code",
        "Voice" => "sung voices: sing(notes, \"lyrics\") in the loops that play it",
        "Instrument" => "a native instrument: the modelled @piano, the modelled @drums kit, a @soundfont program",
        "Loop" => "a loop: plays its instrument from a pattern, code or a note list, once per cycle",
        "Stinger" => "a loop that plays once when triggered",
        "Section" => "a span of bars and the loops that play in it (arrange: [...] in Score)",
        "Lowpass" | "Highpass" | "Bandpass" | "Filter" => "a filter over the loops and FX it reads (cutoff res mix)",
        "Delay" => "an echo in beats (time feedback mix spread)",
        "Reverb" => "a room (room mix damp)",
        "Drive" => "saturation (amount mix tone)",
        "Crush" => "bit and rate reduction (bits down mix)",
        "Chorus" => "a chorus (rate depth mix)",
        "Comp" => "a compressor (threshold ratio attack release knee makeup; key: a sidechain)",
        "Duck" => "pushed down by a key loop (key depth release)",
        "Bus" => "a sum with a gain and a pan",
        "Eq" => "an equaliser: four bands b1..b4, each [@peak|@lowshelf|@highshelf|@highpass|@lowpass Hz dB Q]",
        "Width" => "stereo width and mid/side levels (width 0 mono .. 2), mono lows below bass_mono Hz",
        "Limiter" => "a brickwall limiter (ceiling dBFS, gain); the master has one at Score{ceiling}",
        "ShaderFx" => "an effect written as an audio shader",
        "Score" => "the score itself: its members, label and arrangement",
        _ => return None,
    })
}

/// A field's doc: what it does, its range and default.
pub fn field_doc(ty: &str, field: &str) -> Option<&'static str> {
    Some(match (ty, field) {
        (_, "at") => "the card's place on the graph (written by the Graph view)",
        ("Clock", "bpm") => "tempo, 20..400 (default 120)",
        ("Clock", "sig") => "time signature [beats unit], default [4 4]",
        ("Clock", "swing") => "delay of the off-beat sixteenths, -0.5..0.75 (default 0)",
        ("Clock", "seed") => "the random stream's seed (default 1)",
        ("Clock", "params") => "game params, {name: value}: param(@name) in code",
        ("Clock", "midi") => "{param: cc}: a MIDI controller moves the param, like {cutoff: 74}",
        ("Clock", "osc_in") => "the UDP port OSC arrives on (like 9000): osc(\"/addr\") in code, /param/<name> moves a param",
        ("Clock", "osc_out") => "where osc_send sends, \"host:port\"",
        ("Clock", "tempo") => "{param: @mood from: 64 to: 104 ramp: 4}: the tempo follows a param",
        ("Synth", "preset") => "a recipe preset (@acid @pad @pluck ...)",
        ("Synth", "cutoff") => "filter cutoff in Hz, 20..20000",
        ("Synth", "res") | ("Synth", "q") => "filter resonance (Q), 0.5..20",
        ("Synth", "attack") | ("Synth", "decay") | ("Synth", "release") => "envelope time in seconds",
        ("Synth", "sustain") => "sustain level 0..1",
        ("Synth", "drive") => "saturation 0..20",
        ("Synth", "mono") => "one voice at a time (legato lines)",
        ("Synth", "gain") => "level",
        ("Drums", "kit") => "@analog_80s @analog_hybrid @studio @lofi @chip",
        ("Drums", "tune") => "{drum: semitones}",
        ("Drums", "gain") => "{drum: level}",
        ("Sampler", "src") => "asset(id, rev): a library sample",
        ("Sampler", "root") => "the note the sample plays as recorded (default @c4)",
        ("Ironfish", "program") => "\"init\" \"glass\" \"acid\" \"sub\" \"formant\" \"chip\" \"supersaw\" \"pad\" or 0-7",
        ("Ironfish", _) => "0..1 over Ironfish's own range; unset keeps the program's",
        ("Morph", "preset") => "a Morph preset by name (instrument_search lists them)",
        ("Morph", "mods") => "[[@src @dst amount] ...]: mod rows (morph_params mods)",
        ("Morph", "fx") => "[@delay @reverb @chorus @distortion ...]: the rack, in order (each effect's parameters: morph_params groups [\"fx_<kind>\"])",
        ("Morph", _) => "a Morph parameter (morph_params lists them with ranges)",
        ("Tandem", "patch") => "a library patch: \"Category/Name\" (tandem_patches finds them)",
        ("Tandem", _) => "a Tandem parameter by storage name, raw units (tandem_params lists them)",
        ("Shader", "lib") => "a library shader instrument by name",
        ("Shader", "code") => "the shader's own code (a string)",
        ("Shader", "mono") => "one voice, legato (slides)",
        ("Voice", "profile") => "@soprano @alto @tenor @baritone @bass or asset(id, rev)",
        ("Voice", "choir") => "singers 1..8",
        ("Voice", "parts") => "harmony parts 1..8",
        ("Voice", _) => "a singing style knob",
        ("Instrument", "kind") => "@piano @drums @soundfont @midi (outboard gear) @input (a microphone, a line in)",
        ("Instrument", "channel") => "@midi: the MIDI channel 1..16 (default 1)",
        ("Instrument", "port") => "@midi: the output port's name (default: every output)",
        ("Instrument", "preset") => "the piano's preset",
        ("Instrument", "src") => "asset(id, rev): a library SoundFont (SF2); none: the General MIDI SoundFont",
        ("Instrument", "program") => "General MIDI program 0..127, or a name: @strings @brass @choir ...",
        ("Instrument", "bank") => "SoundFont bank (128 percussion)",
        ("Instrument", "gain") => "level 0..4 (default 1)",
        ("Loop" | "Stinger", "play") => "the instrument node it plays",
        ("Loop" | "Stinger", "beats") => "the cycle's length in beats",
        ("Loop" | "Stinger", "amp") => "level 0..4 (default 0.8)",
        ("Loop" | "Stinger", "pan") => "-1..1",
        ("Loop" | "Stinger", "mute") | ("Loop" | "Stinger", "solo") => "true / false",
        ("Loop" | "Stinger", "sync") => "@bar, @now or another loop: where it starts when added live",
        ("Loop" | "Stinger", "hits") => "a pattern \"x..x\" (x hit, X accent, o ghost, . rest)",
        ("Loop" | "Stinger", "note") => "the drum or note a pattern plays",
        ("Loop" | "Stinger", "step") => "beats per pattern character (default 0.25)",
        ("Loop" | "Stinger", "code") => "fn(i) { ... }: run once per cycle",
        ("Loop" | "Stinger", "when") => "fn(i) { ... }: false rests the cycle",
        ("Loop" | "Stinger", "follow") => "{param: @x from: 0.2 to: 0.8}: fades with a param",
        ("Loop" | "Stinger", "midi_in") => "true or a channel 1..16: a MIDI keyboard plays the loop's instrument live",
        ("Loop" | "Stinger", "dur") => "default note length in beats",
        ("Loop" | "Stinger", "notes") => "[[beat note dur vel] ...]: a note list, streamed",
        ("Stinger", "quant") => "@bar, @now or beats: when a trigger lands",
        ("Section", "bars") => "its length in bars",
        ("Section", "loops") => "the loops that play in it",
        ("Section", "next") => "@name, @self or @stop: what follows",
        ("Section", "repeat") => "true: repeats until a goto",
        ("Section", "bpm") => "its tempo from its first beat",
        ("Section", "ramp") => "beats to reach its bpm",
        (_, "in") => "the loops and FX it reads",
        (_, "mix") => "dry/wet 0..1",
        _ => return None,
    })
}

/// The fields a node type reads, from the model's lists.
pub fn fields_of(ty: &str) -> Vec<&'static str> {
    use crate::model::*;
    match ty {
        "Clock" => CLOCK_FIELDS.to_vec(),
        "Synth" => SYNTH_FIELDS.to_vec(),
        "Drums" => DRUMS_FIELDS.to_vec(),
        "Sampler" => SAMPLER_FIELDS.to_vec(),
        "Ironfish" => IRON_FIELDS.to_vec(),
        "Morph" => morph_fields(),
        "Instrument" => INSTRUMENT_FIELDS.to_vec(),
        "Voice" => VOICE_FIELDS.to_vec(),
        "Loop" | "Stinger" => LOOP_FIELDS.to_vec(),
        "Section" => SECTION_FIELDS.to_vec(),
        "Shader" => vec!["lib", "code", "mono", "at"],
        "ShaderFx" => vec!["in", "lib", "code", "at"],
        other => match makepad_score_player::FxKind::parse(other) {
            Some(kind) => kind.params().iter().map(|(p, ..)| *p).chain(["in", "key", "at", "b1", "b2", "b3", "b4"]).collect(),
            None => Vec::new(),
        },
    }
}

/// The loop code's functions.
pub const FUNCTIONS: [(&str, &str); 63] = [
    ("play", "play(note, {amp pan dur cutoff env cutoff_env ...}): a note or chord now; returns its handle"),
    ("glide", "glide(h, {note by amp cutoff}, beats): a playing note slides there"),
    ("hit", "hit(@kick, {amp tune}): a drum now"),
    ("sample", "sample(note, {amp dur rate start finish slice slices onset looped fit stretch}): the sampler"),
    ("sing", "sing([notes], \"ly-rics\", {dur}): a sung line; returns its length"),
    ("hits", "hits(\"x..x\", @hat, 0.25): a pattern from here"),
    ("sleep", "sleep(beats): move time on"),
    ("tick", "tick(ring): the ring's next element"),
    ("look", "look(ring): the ring's current element"),
    ("ring", "ring(list)"),
    ("at", "at(list, i): wraps"),
    ("choose", "choose(list): a seeded choice"),
    ("shuffle", "shuffle(list)"),
    ("rand", "rand(lo, hi): seeded per cycle"),
    ("irand", "irand(lo, hi): an integer"),
    ("one_in", "one_in(n): true one time in n"),
    ("chance", "chance(p): true with probability p"),
    ("note", "note(@c4): the number"),
    ("chord", "chord(@a2, @m7, inversion)"),
    ("scale", "scale(@d3, @dorian, octaves)"),
    ("euclid", "euclid(hits, steps, rotate)"),
    ("steps", "steps(\"x.o.\"): velocities or nil"),
    ("param", "param(@name): a Clock param"),
    ("cue", "cue(@name): mark a moment other loops can wait for"),
    ("fx", "fx(@reverb, {room mix …}, fn() { … }): the body's notes through their own effect"),
    ("held", "held(): the keys held on the MIDI input, lowest first (arp(held(), @up, 0.25))"),
    ("cc", "cc(74): a MIDI controller's last value, 0..1"),
    ("osc_send", "osc_send(\"/addr\", [args], \"host:port\"): an OSC message on this beat"),
    ("osc", "osc(\"/addr\"): the number that address last brought (Clock osc_in)"),
    ("wait", "wait(@name): on to the next time a loop cues @name; returns the beats waited"),
    ("beat", "beat(): the beat in the cycle"),
    ("cycle", "cycle(): the cycle number"),
    ("floor", "floor(x)"),
    ("ceil", "ceil(x)"),
    ("round", "round(x)"),
    ("abs", "abs(x)"),
    ("min", "min(a, b)"),
    ("max", "max(a, b)"),
    ("clamp", "clamp(x, lo, hi)"),
    ("bar", "bar(): the bar playing, from the piece's start (0)"),
    ("section", "section(): the section playing (its name)"),
    ("section_bar", "section_bar(): the bar within the section"),
    ("rev", "rev(list)"),
    ("rot", "rot(list, n): rotated left by n"),
    ("mirror", "mirror(list): there and back"),
    ("reflect", "reflect(list): there and back, no repeated ends"),
    ("stretch", "stretch(list, n): each element n times"),
    ("rep", "rep(list, n): the list n times"),
    ("first", "first(list, n)"),
    ("last", "last(list, n)"),
    ("drop", "drop(list, n): all but the first n"),
    ("sort", "sort(list)"),
    ("pick", "pick(list, n): n seeded picks"),
    ("line", "line(from, to, n): n evenly spaced values"),
    ("bits", "bits(value, width): its bits as true/false"),
    ("deg", "deg(degree, root, mode): a scale degree (1-based)"),
    ("snap", "snap(note, root, mode): the nearest note of the scale"),
    ("phrase", "phrase(notes, durs, {amp}): a line from here; returns its length"),
    ("arp", "arp(chord, @up | @down | @updown | @random, rate, octaves, steps): returns its length"),
    ("strum", "strum(chord, spread): the notes a hair apart"),
    ("put", "put(@key, value): share a value from this beat on"),
    ("get", "get(@key): the latest value put at or before this beat"),
    ("log", "log(x): shown with the loop"),
];

/// Levenshtein distance, as AS1's shader compiler measures near misses.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let here = row[j];
            row[j] = (row[j] + 1).min(row[j - 1] + 1).min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = here;
        }
    }
    row[b.len()]
}

/// The nearest candidate to `name`, if near enough to be a typo (the rule
/// AS1's compiler uses).
pub fn did_you_mean<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    candidates
        .into_iter()
        .filter(|c| *c != name)
        .map(|c| (edit_distance(name, c), c))
        .filter(|(d, c)| *d <= 2.max(name.len() / 4) && *d < c.len())
        .min()
        .map(|(_, c)| c)
}

/// ", did you mean `x`?" or nothing.
pub fn hint<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> String {
    did_you_mean(name, candidates).map(|c| format!(", did you mean `{c}`?")).unwrap_or_default()
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'#'
}

/// The word around `at` (with a leading `@`): its range.
pub fn word_at(src: &str, at: usize) -> Option<(usize, usize)> {
    let b = src.as_bytes();
    let at = at.min(b.len());
    let mut start = at;
    while start > 0 && is_word(b[start - 1]) {
        start -= 1;
    }
    if start > 0 && b[start - 1] == b'@' {
        start -= 1;
    }
    let mut end = at;
    while end < b.len() && is_word(b[end]) {
        end += 1;
    }
    (end > start).then_some((start, end))
}

/// Where the cursor is: the constructor it is inside (type and name), and
/// the field whose value it is in, if any.
struct Place {
    ty: Option<String>,
    field: Option<String>,
    /// In a statement's value position (`let x = |`).
    after_eq: bool,
    /// Inside a `fn(...) { ... }` body.
    in_code: bool,
}

fn place(src: &str, at: usize) -> Place {
    let tokens = source::lex(&src[..at]);
    let text = |t: &source::Token| &src[t.start..t.end];
    // Open braces with the token before each (a type name, `)` of a fn...).
    let mut stack: Vec<(Option<String>, bool)> = Vec::new();
    let mut field: Option<String> = None;
    let mut expect_field = false;
    let mut last_punct = "";
    // Brackets inside the current value (a list, a call).
    let mut depth = 0i32;
    for (i, t) in tokens.iter().enumerate() {
        if t.kind == Tok::Comment {
            continue;
        }
        let s = text(t);
        match (t.kind, s) {
            (Tok::Punct, "{") => {
                let before = i.checked_sub(1).map(|j| &tokens[j]);
                let is_fn = before.is_some_and(|b| text(b) == ")");
                let ty = before.filter(|b| b.kind == Tok::Ident && text(b).starts_with(|c: char| c.is_ascii_uppercase())).map(|b| text(b).to_string());
                stack.push((ty, is_fn));
                field = None;
                depth = 0;
                expect_field = true;
            }
            (Tok::Punct, "}") => {
                stack.pop();
                field = None;
                expect_field = true;
            }
            (Tok::Ident, name) if expect_field && tokens.get(i + 1).is_some_and(|n| text(n) == ":") => {
                field = Some(name.to_string());
                expect_field = false;
            }
            (Tok::Punct, ":") => {}
            (Tok::Punct, "[" | "(") => depth += 1,
            (Tok::Punct, "]" | ")") => {
                depth -= 1;
                if depth <= 0 {
                    depth = 0;
                    field = None;
                    expect_field = !stack.last().is_some_and(|(_, is_fn)| *is_fn);
                }
            }
            _ => {
                // A value ended: the next word may be a field again.
                if depth == 0 && matches!(t.kind, Tok::Num | Tok::Str | Tok::Id | Tok::Ident) && !tokens.get(i + 1).is_some_and(|n| matches!(text(n), "(" | "." )) {
                    field = None;
                    expect_field = !stack.last().is_some_and(|(_, is_fn)| *is_fn);
                }
            }
        }
        if t.kind == Tok::Punct {
            last_punct = s;
        }
    }
    let in_code = stack.iter().any(|(_, is_fn)| *is_fn);
    let ty = stack.iter().rev().find_map(|(ty, _)| ty.clone());
    let after_eq = stack.is_empty() && last_punct == "=";
    // The word being typed is not a finished field key.
    let typing_key = field.is_some() && tokens.last().is_some_and(|t| text(t) == ":");
    Place { ty, field: if typing_key || field.is_some() { field } else { None }, after_eq, in_code }
}

/// Completions at `at` (the cursor's byte offset) in `src`.
pub fn complete(src: &str, at: usize, available: &Available) -> Option<Completions> {
    let at = at.min(src.len());
    let b = src.as_bytes();
    let mut start = at;
    while start > 0 && is_word(b[start - 1]) {
        start -= 1;
    }
    let atom = start > 0 && b[start - 1] == b'@';
    if atom {
        start -= 1;
    }
    let prefix = &src[start..at];
    // Only while a word is being typed (or right after `@`).
    if prefix.is_empty() {
        return None;
    }
    let lower = prefix.trim_start_matches('@').to_ascii_lowercase();
    let here = place(src, start);
    let mut items: Vec<Item> = Vec::new();
    let item = |label: &str, insert: String, detail: String, kind| Item { label: label.to_string(), insert, detail, kind };
    let outline = source::outline(src);
    if atom {
        let atoms = |items: &mut Vec<Item>, names: &mut dyn Iterator<Item = String>, detail: &str| {
            for name in names {
                items.push(Item { label: format!("@{name}"), insert: crate::instruments::atom(&name), detail: detail.to_string(), kind: ItemKind::Atom });
            }
        };
        match (here.ty.as_deref(), here.field.as_deref()) {
            (Some("Synth"), Some("preset")) => atoms(&mut items, &mut crate::palette::sounds::sounds().patch_names().into_iter().map(str::to_string), "synth patch"),
            (Some("Drums"), Some("kit")) => atoms(&mut items, &mut crate::palette::sounds::sounds().kit_names().into_iter().map(str::to_string), "drum kit"),
            (Some("Instrument"), Some("kind")) => atoms(&mut items, &mut ["piano", "drums", "soundfont"].iter().map(|s| s.to_string()), "native instrument"),
            (Some("Voice"), Some("profile")) => atoms(&mut items, &mut ["soprano", "alto", "tenor", "baritone", "bass"].iter().map(|s| s.to_string()), "singer"),
            (Some("Loop" | "Stinger"), Some("sync")) => atoms(&mut items, &mut ["bar", "now"].iter().map(|s| s.to_string()), "start"),
            (Some("Section"), Some("next")) => {
                let mut names = outline.nodes.iter().filter(|n| n.type_name == "Section").map(|n| n.name.clone()).chain(["self".to_string(), "stop".to_string()]);
                atoms(&mut items, &mut names, "section");
            }
            _ => {
                let mut drums = presets::DRUM_SLOTS.iter().map(|s| s.to_string()).chain(["pedalhat", "floortom", "tom1", "tom2", "tom3", "bell"].iter().map(|s| s.to_string()));
                atoms(&mut items, &mut drums, "drum");
                atoms(&mut items, &mut crate::theory::CHORD_KINDS.iter().map(|s| s.to_string()), "chord");
                atoms(&mut items, &mut crate::theory::SCALE_KINDS.iter().map(|s| s.to_string()), "scale");
                if lower.len() >= 1 {
                    for octave in 0..8 {
                        let mut notes = crate::theory::NAMES.iter().map(|n| format!("{n}{octave}"));
                        atoms(&mut items, &mut notes, "note");
                    }
                }
                atoms(&mut items, &mut crate::palette::sounds::sounds().patch_names().into_iter().map(str::to_string), "preset");
            }
        }
    } else if here.after_eq {
        for ty in crate::vm::NODE_TYPES {
            items.push(item(ty, format!("{ty}{{}}"), type_doc(ty).unwrap_or("").to_string(), ItemKind::NodeType));
        }
    } else if here.in_code {
        for (name, doc) in FUNCTIONS {
            items.push(item(name, name.to_string(), doc.to_string(), ItemKind::Function));
        }
        for n in outline.nodes.iter().filter(|n| n.type_name.is_empty() || n.type_name == "fn") {
            items.push(item(&n.name, n.name.clone(), "a helper".into(), ItemKind::Node));
        }
    } else if let Some(ty) = here.ty.as_deref() {
        match here.field.as_deref() {
            Some("play") => {
                for n in outline.nodes.iter().filter(|n| INSTRUMENT_TYPES.contains(&n.type_name.as_str())) {
                    items.push(item(&n.name, n.name.clone(), format!("{}: {}", n.type_name, type_doc(&n.type_name).unwrap_or("")), ItemKind::Node));
                }
            }
            Some("in" | "loops" | "key") => {
                for n in outline.nodes.iter().filter(|n| n.type_name == "Loop" || n.type_name == "Stinger" || fields_of(&n.type_name).contains(&"in")) {
                    items.push(item(&n.name, n.name.clone(), n.type_name.clone(), ItemKind::Node));
                }
            }
            Some("program") if ty == "Instrument" => {
                for (i, name) in GM_PROGRAMS.iter().enumerate() {
                    items.push(item(&format!("{i} {name}"), i.to_string(), "General MIDI program".into(), ItemKind::Atom));
                }
            }
            Some("lib") => {
                let effect = ty == "ShaderFx";
                for (name, code) in makepad_score_player::shader::LIBRARY {
                    if code.contains("fn voice") != effect {
                        items.push(item(name, format!("{name:?}"), "library shader".into(), ItemKind::Atom));
                    }
                }
            }
            Some("src") => {
                let list = if ty == "Instrument" { &available.fonts } else { &available.samples };
                for (title, a) in list {
                    items.push(item(title, format!("asset({:?}, {:?})", a.id, a.rev), "library asset".into(), ItemKind::Atom));
                }
            }
            _ => {
                for field in fields_of(ty) {
                    items.push(item(field, format!("{field}: "), field_doc(ty, field).unwrap_or("").to_string(), ItemKind::Field));
                }
            }
        }
    } else {
        for ty in crate::vm::NODE_TYPES {
            items.push(item(ty, ty.to_string(), type_doc(ty).unwrap_or("").to_string(), ItemKind::NodeType));
        }
    }
    let key = |label: &str| label.trim_start_matches('@').to_ascii_lowercase();
    let mut first: Vec<Item> = items.iter().filter(|i| key(&i.label).starts_with(&lower)).cloned().collect();
    let mut rest: Vec<Item> = items.into_iter().filter(|i| !key(&i.label).starts_with(&lower) && key(&i.label).contains(&lower)).collect();
    first.dedup_by(|a, b| a.label == b.label);
    first.append(&mut rest);
    first.truncate(MAX_ITEMS);
    // Nothing to offer, or only what is already there.
    if first.is_empty() || (first.len() == 1 && first[0].insert == prefix) {
        return None;
    }
    Some(Completions { start, end: at, items: first })
}

/// Docs for the word at `at`: its range and a line or two.
pub fn hover(src: &str, at: usize) -> Option<((usize, usize), String)> {
    let (start, end) = word_at(src, at)?;
    let word = &src[start..end];
    let here = place(src, start);
    if let Some(doc) = type_doc(word) {
        let fields = fields_of(word);
        return Some(((start, end), format!("{word}: {doc}\nfields: {}", fields.join(" "))));
    }
    if let Some(ty) = here.ty.as_deref() {
        if src[end..].trim_start().starts_with(':') {
            if let Some(doc) = field_doc(ty, word) {
                return Some(((start, end), format!("{ty}.{word}: {doc}")));
            }
        }
    }
    let outline = source::outline(src);
    if let Some(node) = outline.node(word) {
        let fields: Vec<String> = node.fields.iter().filter(|f| !f.bare && f.key != "code" && f.key != "notes").map(|f| format!("{}: {}", f.key, f.value.text(src).chars().take(24).collect::<String>())).collect();
        return Some(((start, end), format!("{word} = {}{{{}}}\n{}", node.type_name, fields.join(" "), type_doc(&node.type_name).unwrap_or(""))));
    }
    if let Some((_, doc)) = FUNCTIONS.iter().find(|(name, _)| *name == word) {
        return Some(((start, end), doc.to_string()));
    }
    let bare = word.trim_start_matches('@');
    if word.starts_with('@') {
        if let Some(patch) = crate::palette::sounds::sounds().patch(bare) {
            return Some(((start, end), format!("synth patch {bare}: {}", patch.about)));
        }
        if let Some(kit) = crate::palette::sounds::sounds().kit(bare) {
            return Some(((start, end), format!("drum kit {bare}: {}", kit.about)));
        }
        if let Some(gm) = presets::gm_drum(bare) {
            return Some(((start, end), format!("drum {bare} (GM note {gm})")));
        }
        if let Some(n) = crate::theory::note_number(bare) {
            return Some(((start, end), format!("note {bare} = MIDI {n}")));
        }
        if crate::theory::chord_kind(bare).is_some() {
            return Some(((start, end), format!("chord kind {bare}")));
        }
        if crate::theory::scale_kind(bare).is_some() {
            return Some(((start, end), format!("scale {bare}")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet bass = Synth{preset: @ac cutoff: 700}\nlet line = Loop{play: ba beats: 2 code: fn(i) { pl(@c4) sleep(1) }}\nScore{clock bass line}\n";

    fn labels(src: &str, marker: &str) -> Vec<String> {
        let at = src.find(marker).unwrap() + marker.len();
        complete(src, at, &Available::default()).map(|c| c.items.into_iter().map(|i| i.label).collect()).unwrap_or_default()
    }

    #[test]
    fn completions_follow_the_cursor() {
        assert_eq!(labels(SRC, "preset: @ac").first().map(String::as_str), Some("@acid"));
        assert_eq!(labels(SRC, "play: ba"), ["bass"]);
        assert!(labels(SRC, "{ pl").contains(&"play".to_string()));
        let typed = "use mod.score.*\nlet x = Syn";
        assert_eq!(labels(typed, "= Syn").first().map(String::as_str), Some("Synth"));
        let field = "use mod.score.*\nlet l = Loop{play: s be";
        assert!(labels(field, "s be").contains(&"beats".to_string()));
        let kind = "use mod.score.*\nlet g = Instrument{kind: @p";
        assert_eq!(labels(kind, "@p"), ["@piano"]);
        // Fast enough for every keystroke on a long score.
        let long = SRC.repeat(200);
        let t = std::time::Instant::now();
        for _ in 0..10 {
            complete(&long, long.len() - 30, &Available::default());
        }
        assert!(t.elapsed().as_secs_f64() / 10.0 < 0.004, "{:?}", t.elapsed() / 10);
    }

    #[test]
    fn hover_docs_and_near_misses() {
        let at = SRC.find("cutoff").unwrap() + 2;
        assert!(hover(SRC, at).unwrap().1.contains("cutoff in Hz"));
        let at = SRC.find("clock bass").unwrap() + 7;
        assert!(hover(SRC, at).unwrap().1.starts_with("bass = Synth{"), "{:?}", hover(SRC, at));
        assert_eq!(did_you_mean("Synht", crate::vm::NODE_TYPES), Some("Synth"));
        assert_eq!(did_you_mean("cutof", ["cutoff", "res"]), Some("cutoff"));
        assert_eq!(did_you_mean("zzz", ["cutoff", "res"]), None);
    }
}
