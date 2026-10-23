//! The library's AI tools, defined once for every host (Stage's control
//! API, and the MCP server `midi_library mcp` the Batch director is given):
//! `midi_lookup` finds songs, `midi_transcribe` reads one as the text
//! transcription ([`crate::roll`]) at the detail asked, `score_text` reads
//! a Score in the same form. The model never sees MIDI bytes or event
//! lists.

use crate::index::{self, Entry};
use crate::import::{self, About, Options};
use crate::roll::{self, Detail};
use makepad_strict_json::{self as json, Value};

/// One tool: name, description, JSON Schema of its arguments.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input: &'static str,
}

pub const LOOKUP: ToolDef = ToolDef {
    name: "midi_lookup",
    description: "Find real songs in the song library (installed from the app's Resources: the MIDI set, and the chip and tracker collections such as the C64 SID collection; the songs are their owners' works: read them to learn, never copy a melody): `query` by decade (80s, 1980s, 1983-1986), genre or tag (synthpop, new wave, disco, italo, chiptune, demo, game), artist or composer, title or game words, format or machine (sid, mod, xm, s3m, it; c64, amiga, pc), `artist:name`, `quality:60` (0..100: a full arrangement scores high). Words that match nothing rank a song lower, not out. Each song: id, artist, title, year, genres, tags, bpm, key, seconds, roles, quality, and for chip music its format, machine and game. Read one with midi_transcribe (a chip tune's notes carry their arpeggios, vibrato, slides and filter sweeps as note options).",
    input: r#"{"type": "object", "properties": {"query": {"type": "string", "minLength": 1, "description": "e.g. 80s synthpop"}, "n": {"type": "integer", "minimum": 1, "maximum": 50, "description": "how many (default 12)"}}, "required": ["query"]}"#,
};

pub const TRANSCRIBE: ToolDef = ToolDef {
    name: "midi_transcribe",
    description: "Read a library song (`song`: an id from midi_lookup, or \"Artist - Title\") as a compact text transcription: a header (tempo, meter, key, form, the parts with their roles and instruments), then per bar a chord line (a chord a beat) and each part on a sixteenth grid (`A4 ~ ~ .`: a note starts, holds, rests; `A3+C#4+E4` a chord; `!` loud, `?` soft; drums one row per drum, `x` hit `X` accent `o` ghost); repeated bars as `7-8 = 5-6`. `detail`: full (default), chords, drums, melody or bass; `bars` \"1-16\"; `parts` names or roles (\"bass,drums\"). Study form, grooves, voicings and arrangement; never copy a melody.",
    input: r#"{"type": "object", "properties": {"song": {"type": "string", "minLength": 1, "description": "an id from midi_lookup, or Artist - Title"}, "detail": {"type": "string", "enum": ["full", "chords", "drums", "melody", "bass"], "description": "what to show (default full)"}, "bars": {"type": "string", "description": "e.g. 1-16 (default all)"}, "parts": {"type": "string", "description": "part names or roles, comma separated"}}, "required": ["song"]}"#,
};

pub const SCORE_TEXT: ToolDef = ToolDef {
    name: "score_text",
    description: "Read a Score (default: the score open in the app, or `source`) in the same text transcription midi_transcribe gives for real songs, by playing its loops: compare your song with a reference like for like. `detail`, `bars` and `parts` as in midi_transcribe.",
    input: r#"{"type": "object", "properties": {"source": {"type": "string", "description": "a score's Splash source (default: the open score)"}, "detail": {"type": "string", "enum": ["full", "chords", "drums", "melody", "bass"], "description": "what to show (default full)"}, "bars": {"type": "string", "description": "e.g. 1-16"}, "parts": {"type": "string", "description": "part names or roles"}}}"#,
};

pub const FIND_ONLINE: ToolDef = ToolDef {
    name: "song_find_online",
    description: "Search an online music archive for one song the person wants (e.g. Space Debris, Aryx): the archive's own search, one request; nothing is downloaded. `source`: modarchive (The Mod Archive: MOD, S3M, XM and IT modules). Each hit: id, title, file, format. Download the one the person picks with song_download.",
    input: r#"{"type": "object", "properties": {"query": {"type": "string", "minLength": 2, "description": "a title or file name"}, "source": {"type": "string", "enum": ["modarchive"], "description": "default modarchive"}}, "required": ["query"]}"#,
};

pub const DOWNLOAD: ToolDef = ToolDef {
    name: "song_download",
    description: "Download one song the person picked from song_find_online (`source`, `id`) from the archive itself, with its credits and licence, into the song library (then midi_lookup finds it and midi_transcribe reads it). Only what the person asked for: never download in bulk. Returns the song with its artist, licence and the archive's terms; tell the person both.",
    input: r#"{"type": "object", "properties": {"id": {"type": "string", "minLength": 1}, "source": {"type": "string", "enum": ["modarchive"]}, "file": {"type": "string", "description": "the hit's file name"}, "title": {"type": "string"}}, "required": ["id", "file"]}"#,
};

pub const TOOLS: [ToolDef; 5] = [LOOKUP, TRANSCRIBE, SCORE_TEXT, FIND_ONLINE, DOWNLOAD];

/// The detail a call asks for.
pub fn detail_of(args: &Value) -> Detail {
    let mut d = Detail::named(args.get("detail").and_then(Value::as_str).unwrap_or("full"));
    if let Some(parts) = args.get("parts").and_then(Value::as_str).filter(|p| !p.trim().is_empty()) {
        d.parts = Some(parts.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect());
    }
    if let Some(bars) = args.get("bars").and_then(Value::as_str) {
        let (a, b) = bars.split_once('-').unwrap_or((bars, bars));
        if let (Ok(a), Ok(b)) = (a.trim().parse(), b.trim().parse()) {
            d.bars = Some((a, b));
        }
    }
    d
}

pub fn about(e: &Entry) -> About {
    About { title: e.title.clone(), artist: e.artist.clone(), year: e.year, id: e.id.clone(), path: e.path.clone(), genre: e.genres.join(","), tags: e.tags.clone() }
}

pub fn song_json(e: &Entry) -> Value {
    let f = &e.features;
    json::obj(vec![
        ("id", json::s(e.id.clone())),
        ("artist", json::s(e.artist.clone())),
        ("title", json::s(e.title.clone())),
        ("year", e.year.map_or(Value::Null, |y| Value::Int(y as i64))),
        ("genres", Value::Arr(e.genres.iter().map(|g| json::s(g.clone())).collect())),
        ("tags", Value::Arr(e.tags.iter().take(8).map(|t| json::s(t.0.clone())).collect())),
        ("bpm", Value::F64(f.bpm)),
        ("key", json::s(f.key.clone())),
        ("seconds", Value::F64(f.seconds)),
        ("roles", Value::Arr(f.roles.iter().map(|r| json::s(r.name())).collect())),
        ("quality", Value::Int(f.quality as i64)),
        ("format", json::s(e.format.clone())),
        ("machine", json::s(e.platform.clone())),
        ("game", json::s(e.game.clone())),
    ])
}

/// A library song's entry and MIDI bytes, by id or "Artist - Title".
pub fn song(entries: &[Entry], name: &str) -> Result<(Entry, Vec<u8>), String> {
    let entry = index::find(entries, name).ok_or_else(|| format!("no song `{name}` in the library (an id from midi_lookup, or Artist - Title)"))?.clone();
    let bytes = index::bytes(&entry)?;
    Ok((entry, bytes))
}

/// A library song as an arrangement and its Score: MIDI through the MIDI
/// importer, chip and tracker music through [`crate::chip::import`].
pub fn arrangement(entry: &Entry) -> Result<(crate::arrangement::Arrangement, String), String> {
    if entry.format != "mid" {
        let imported = crate::chip::import::import_entry(entry)?;
        return Ok((imported.arrangement, imported.source));
    }
    let bytes = index::bytes(entry)?;
    let imported = import::import(&bytes, &about(entry), &Options::default())?;
    Ok((imported.arrangement, imported.source))
}

fn library() -> Result<Vec<Entry>, String> {
    index::load(&index::index_path()).map_err(|e| format!("the MIDI library is not set up on this machine ({e}): run `midi_library install --accept` and `midi_library index`"))
}

/// Run a tool; `open` is the app's open score (for `score_text` without a
/// source). `Err` is a refusal to show the model.
pub fn call(name: &str, args: &Value, open: Option<&str>) -> Result<Value, String> {
    match name {
        "midi_lookup" => {
            let entries = library()?;
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let n = args.get("n").and_then(Value::as_u64).unwrap_or(12).clamp(1, 50) as usize;
            let songs = index::search(&entries, query, n).into_iter().map(|(e, _)| song_json(e)).collect();
            Ok(json::obj(vec![("songs", Value::Arr(songs)), ("library", Value::Int(entries.len() as i64))]))
        }
        "midi_transcribe" => {
            let entries = library()?;
            let name = args.get("song").and_then(Value::as_str).unwrap_or("");
            let entry = index::find(&entries, name).ok_or_else(|| format!("no song `{name}` in the library (an id from midi_lookup, or Artist - Title)"))?.clone();
            let (arrangement, _) = arrangement(&entry)?;
            Ok(json::obj(vec![("song", json::s(entry.label())), ("text", json::s(roll::to_text(&arrangement, &detail_of(args))))]))
        }
        "score_text" => {
            let source = args.get("source").and_then(Value::as_str).or(open).ok_or("give the score's `source` (no score is open)")?;
            let a = roll::from_score(source)?;
            Ok(json::obj(vec![("text", json::s(roll::to_text(&a, &detail_of(args))))]))
        }
        "song_find_online" => {
            let source = crate::chip::web::SOURCES.iter().find(|s| Some(s.id) == args.get("source").and_then(Value::as_str).or(Some("modarchive"))).ok_or("no such source (modarchive)")?;
            let hits = crate::chip::web::search(source, args.get("query").and_then(Value::as_str).unwrap_or(""))?;
            let hits = hits.into_iter().take(25).map(|h| json::obj(vec![("id", json::s(h.id)), ("title", json::s(h.title)), ("file", json::s(h.file)), ("format", json::s(h.format))])).collect();
            Ok(json::obj(vec![("source", json::s(source.name)), ("terms", json::s(source.terms)), ("hits", Value::Arr(hits))]))
        }
        "song_download" => {
            let source = crate::chip::web::SOURCES.iter().find(|s| Some(s.id) == args.get("source").and_then(Value::as_str).or(Some("modarchive"))).ok_or("no such source (modarchive)")?;
            let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let hit = crate::chip::web::Hit { source: source.id, id: text("id"), title: text("title"), file: text("file"), format: String::new() };
            let entry = crate::chip::web::download(&hit)?;
            let mut out = song_json(&entry);
            if let Value::Obj(fields) = &mut out {
                fields.push(("credit".into(), json::s(entry.tid.clone())));
                fields.push(("terms".into(), json::s(source.terms)));
            }
            Ok(out)
        }
        _ => Err(format!("no tool `{name}`")),
    }
}

/// The MCP server on stdio (JSON-RPC 2.0, a message per line): these tools
/// for an MCP client such as the Batch director's CLI.
pub fn serve_mcp() {
    use std::io::{BufRead, Write};
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = makepad_strict_json::parse(line.trim().as_bytes()) else { continue };
        let Some(id) = msg.get("id").cloned() else { continue };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(Value::Obj(Vec::new()));
        let result = match method {
            "initialize" => Ok(json::obj(vec![
                ("protocolVersion", json::s(params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2025-06-18"))),
                ("capabilities", json::obj(vec![("tools", json::obj(vec![("listChanged", Value::Bool(false))]))])),
                ("serverInfo", json::obj(vec![("name", json::s("makepad-midi-library")), ("version", json::s("0.1.0"))])),
                ("instructions", json::s("Real songs as text: midi_lookup finds them, midi_transcribe reads one (chords, drums, parts on a sixteenth grid). For learning arrangement and style; never copy a melody.")),
            ])),
            "ping" => Ok(json::obj(vec![])),
            "tools/list" => Ok(json::obj(vec![("tools", Value::Arr(TOOLS.iter().map(|t| json::obj(vec![
                ("name", json::s(t.name)),
                ("description", json::s(t.description)),
                ("inputSchema", makepad_strict_json::parse(t.input.as_bytes()).unwrap_or(Value::Obj(Vec::new()))),
            ])).collect()))])),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(Value::Obj(Vec::new()));
                let (text, error) = match call(name, &args, None) {
                    Ok(v) => (v.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| v.to_json()), false),
                    Err(e) => (e, true),
                };
                Ok(json::obj(vec![("content", Value::Arr(vec![json::obj(vec![("type", json::s("text")), ("text", json::s(text))])])), ("isError", Value::Bool(error))]))
            }
            _ => Err(format!("no method `{method}`")),
        };
        let reply = match result {
            Ok(result) => json::obj(vec![("jsonrpc", json::s("2.0")), ("id", id), ("result", result)]),
            Err(message) => json::obj(vec![("jsonrpc", json::s("2.0")), ("id", id), ("error", json::obj(vec![("code", Value::Int(-32601)), ("message", json::s(message))]))]),
        };
        if writeln!(stdout, "{}", reply.to_json()).is_err() || stdout.flush().is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn schemas_parse_and_score_text_reads_a_score() {
        for t in super::TOOLS {
            makepad_strict_json::parse(t.input.as_bytes()).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        }
        let src = "use mod.score.*\nlet clock = Clock{bpm: 120}\nlet kit = Drums{kit: @analog_hybrid}\nlet bass = Synth{preset: @sub}\nlet kick = Loop{play: kit hits: \"x...x...x...x...\"}\nlet line = Loop{play: bass beats: 4 code: fn(i) { play(@a1, {dur: 0.5}) sleep(1) play(@c2, {dur: 0.5}) sleep(3) }}\nlet a = Section{bars: 2 loops: [kick line]}\nScore{label: \"Test\" arrange: [a] clock kit bass kick line a}";
        let out = super::call("score_text", &makepad_strict_json::Value::Obj(Vec::new()), Some(src)).unwrap();
        let text = out.get("text").and_then(makepad_strict_json::Value::as_str).unwrap();
        assert!(text.contains("K X... X... X... X...") && text.contains("A1! ~ . . C2! ~ .") && text.contains("2 = 1"), "{text}");
    }
}
