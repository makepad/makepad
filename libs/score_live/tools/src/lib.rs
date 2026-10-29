//! The Score tools' text and shape: `score_guide`, `score_check` and
//! `score_listen`, defined once and listed byte for byte the same by every
//! app's AI registry (Stage's studio API, Sandbox's game chat).
//! Dependency-free, so a light registry such as an MCP bridge can list
//! them; running them (and the guide's text) is `makepad-score-live`'s
//! `tools` module.

/// One tool as every registry declares it. `input` and `output` are JSON
/// Schema (strict JSON); `args_doc` is a call sketch for registries that
/// show one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreToolDef {
    /// The dotted canonical name (`score.check`).
    pub name: &'static str,
    /// The underscore spelling (`score_check`): what the model sees in
    /// every app, and what the guide calls the tools.
    pub api_name: &'static str,
    pub description: &'static str,
    pub input: &'static str,
    pub output: &'static str,
    pub args_doc: &'static str,
}

pub const GUIDE: ScoreToolDef = ScoreToolDef {
    name: "score.guide",
    api_name: "score_guide",
    description: "How to write a score: the document (Clock, instruments, Loops with code, FX, Sections, Score{…}), the functions loop code calls, instrument presets and drum kits, genre templates, drum patterns and progressions, arrangement and mixing rules, how to fit a video or a game, and synth panels. Read it once before composing.",
    input: r#"{"type": "object", "properties": {}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"guide": {"type": "string", "description": "the composer guide"}}, "required": ["guide"]}"#,
    args_doc: r#"{}"#,
};

pub const CHECK: ScoreToolDef = ScoreToolDef {
    name: "score.check",
    api_name: "score_check",
    description: "Check a score's Splash source without playing it (default: the score open in the app): every problem with its line and column; once it loads, what it declares (label, bpm, params, sections, arrangement, stingers, loops) and the problems its loops raise over their first 8 bars (a code error shows only when the loop runs).",
    input: r#"{"type": "object", "properties": {"source": {"type": "string", "description": "the score's Splash source (default: the open score)"}}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"ok": {"type": "boolean", "description": "no problems"}, "problems": {"type": "array", "items": {"type": "object", "description": "{line, col, message, text}"}, "description": "the document's or its loops' problems"}}, "required": ["ok", "problems"]}"#,
    args_doc: r#"{"source": "use mod.score.*\n..."}"#,
};

pub const LISTEN: ScoreToolDef = ScoreToolDef {
    name: "score.listen",
    api_name: "score_listen",
    description: "Hear a score as data (you cannot hear audio): renders `bars` bars from `from_bar` offline (default: the score open in the app, or `source`), exactly as it plays, with `params` set and `held` fields moved from a bar on (what an editor panel's control does live: Synth fields, Shader params, Loop amp/pan, FX settings), and measures it: peak and RMS dBFS per loop and FX and the master; the master's spectrum in ten octave bands with a verdict on balance (muddy, harsh, thin, no low end); onsets per bar on the sixteenth grid; per loop the notes it played (beat, note name, velocity); per bar the master's RMS and brightness (bright_hz: the spectrum's centroid), so a held control's effect shows bar by bar; and silence or clipping warnings. Use it after every change to check what you made, and to check an editor panel's ranges: hold a field at its control's ends and hear the difference.",
    input: r#"{"type": "object", "properties": {"source": {"type": "string", "description": "the score's Splash source (default: the open score)"}, "from_bar": {"type": "integer", "minimum": 0, "maximum": 10000, "description": "first bar (default 0)"}, "bars": {"type": "integer", "minimum": 1, "maximum": 64, "description": "how many bars (default 4)"}, "params": {"type": "object", "additionalProperties": {"type": "number"}, "description": "Clock params to set, e.g. {intensity: 0.8}"}, "held": {"type": "array", "items": {"type": "object", "properties": {"path": {"type": "string", "description": "node.field, e.g. mono.cutoff"}, "value": {"type": "number", "description": "the value held"}, "at_bar": {"type": "number", "description": "from this bar of the render (default 0)"}}, "required": ["path", "value"]}, "description": "fields held live during the render, as an editor panel's controls"}}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"ok": {"type": "boolean", "description": "it loaded and rendered"}}, "required": ["ok"]}"#,
    args_doc: r#"{"source": "...", "from_bar": 0, "bars": 4, "params": {"intensity": 0.8}, "held": [{"path": "mono.cutoff", "value": 3000, "at_bar": 2}]}"#,
};

pub const INSTRUMENT_SEARCH: ScoreToolDef = ScoreToolDef {
    name: "instrument.search",
    api_name: "instrument_search",
    description: "Find the instruments and drum kits that fit a description (\"thin bright 80s drum machine\", \"warm upright jazz bass\", \"lush 70s string pad\"): the General MIDI SoundFont's programs and its drum kits (standard, room, power, electronic, analog drum machine, jazz, brush, orchestra), each tagged by family, role, era, genre and character, and measured offline (brightness, attack, decay, low/mid/high energy, loudness; kits by their kick, snare and hats). Each answer has the Score code that plays it (`Instrument{kind: @soundfont ...}`), its words and its measured level against its family (a gain to match). Use it before choosing a sound; prefer what it finds over a generic preset.",
    input: r#"{"type": "object", "properties": {"query": {"type": "string", "description": "what it should sound like: era, genre, instrument, character"}, "kind": {"type": "string", "enum": ["any", "drums", "melodic"], "description": "drum kits only, melodic instruments only, or both (default)"}, "role": {"type": "string", "enum": ["bass", "chords", "lead", "pad", "arp", "texture", "hits", "fx", "drums"], "description": "the part it plays"}, "limit": {"type": "integer", "minimum": 1, "maximum": 20, "description": "how many (default 6)"}}, "required": ["query"]}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"results": {"type": "array", "items": {"type": "object", "description": "{id, name, code, family, roles, words, measured: {centroid_hz, attack_ms, decay_ms, low, mid, high, lufs}, gain}"}, "description": "best first"}}, "required": ["results"]}"#,
    args_doc: r#"{"query": "thin bright 80s drum machine", "kind": "drums", "limit": 4}"#,
};

pub const MORPH_PARAMS: ScoreToolDef = ScoreToolDef {
    name: "morph.params",
    api_name: "morph_params",
    description: "Morph, the wavetable synth node (`Morph{…}`): its parameters, one line each (`key range [log] =default [~ if a mod row may move it]: what it does`), grouped by module (osc_a osc_b osc_c sub noise filter1 filter2 filters env1-4 lfo1-8 macros voice arp fx_*), its mod sources and row syntax, and its presets. Ask for some groups (`osc_*` matches a prefix) to keep it short.",
    input: r#"{"type": "object", "properties": {"groups": {"type": "array", "items": {"type": "string", "description": "a group, or a prefix with *"}, "description": "only these groups (default: all)"}}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"structure": {"type": "string", "description": "how a Morph is written"}, "params": {"type": "string", "description": "one line a parameter, by group"}, "mods": {"type": "string", "description": "sources and row syntax"}, "presets": {"type": "array", "items": {"type": "string", "description": "name: what it is"}, "description": "the built-in patches"}, "groups": {"type": "array", "items": {"type": "string", "description": "a group name"}, "description": "every group"}}, "required": ["params"]}"#,
    args_doc: r#"{"groups": ["osc_a", "lfo1"]}"#,
};

pub const TANDEM_PARAMS: ScoreToolDef = ScoreToolDef {
    name: "tandem.params",
    api_name: "tandem_params",
    description: "Tandem, the hybrid synth node that plays the hybrid patch library (`Tandem{patch: \"Leads/Name\" a_filter1_cutoff: 12}`): how it is written, its parameters one line each (`storage_name range =default: meaning`, raw units) for some groups (a_scene a_osc1 a_mixer a_filters a_env1 a_lfo0 ... b_* fx1..fx16 global; a prefix with * matches), what an oscillator type's param0..6 mean (`osc_type`), what an effect type's p0..p11 mean (`fx_type`), and the type lists (filters with subtypes, waveshapers, effects, mod sources).",
    input: r#"{"type": "object", "properties": {"groups": {"type": "array", "items": {"type": "string", "description": "a group, or a prefix with *"}, "description": "only these groups (default: none, just the structure and type lists)"}, "osc_type": {"type": "integer", "minimum": 0, "maximum": 11, "description": "describe this oscillator type's slots"}, "fx_type": {"type": "integer", "minimum": 0, "maximum": 31, "description": "describe this effect type's slots"}}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"structure": {"type": "string", "description": "how a Tandem is written"}, "params": {"type": "string", "description": "one line a parameter, by group"}, "types": {"type": "string", "description": "the type lists"}, "osc": {"type": "string", "description": "the oscillator type's slots"}, "fx": {"type": "string", "description": "the effect type's slots"}, "groups": {"type": "array", "items": {"type": "string", "description": "a group name"}, "description": "every group"}}, "required": ["structure"]}"#,
    args_doc: r#"{"groups": ["a_filters"], "osc_type": 2}"#,
};

pub const TANDEM_PATCHES: ScoreToolDef = ScoreToolDef {
    name: "tandem.patches",
    api_name: "tandem_patches",
    description: "Find patches of the hybrid patch library (an opt-in download in Resources) for `Tandem{patch: …}`: by words in their category and name (\"bass\", \"pad warm\", \"Jacky Ligon\"), best first, each as `Category/Name` with its category. Without a query: the categories and their patch counts. Says so when the library is not installed.",
    input: r#"{"type": "object", "properties": {"query": {"type": "string", "description": "words to match in category and name"}, "limit": {"type": "integer", "minimum": 1, "maximum": 100, "description": "how many (default 20)"}}}"#,
    output: r#"{"type": "object", "additionalProperties": true, "properties": {"installed": {"type": "boolean", "description": "whether a library is installed"}, "patches": {"type": "array", "items": {"type": "string", "description": "Category/Name"}, "description": "best first"}, "categories": {"type": "array", "items": {"type": "string", "description": "category: count"}, "description": "when no query"}}, "required": ["installed"]}"#,
    args_doc: r#"{"query": "warm pad", "limit": 10}"#,
};

/// Every Score tool, in the order registries list them.
pub const TOOLS: [ScoreToolDef; 7] = [GUIDE, CHECK, LISTEN, INSTRUMENT_SEARCH, MORPH_PARAMS, TANDEM_PARAMS, TANDEM_PATCHES];

/// The Score tool named `name`, dotted or underscored.
pub fn find(name: &str) -> Option<&'static ScoreToolDef> {
    TOOLS.iter().find(|tool| tool.name == name || tool.api_name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_legal_and_schemas_parse() {
        for tool in TOOLS {
            assert_eq!(tool.api_name, tool.name.replace('.', "_"));
            assert!(tool.api_name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}", tool.api_name);
            for schema in [tool.input, tool.output, tool.args_doc] {
                makepad_strict_json::parse(schema.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}: {schema}", tool.name));
            }
            assert_eq!(find(tool.name), find(tool.api_name));
        }
    }
}
