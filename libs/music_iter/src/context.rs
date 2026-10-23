//! MUSIC-AGENTS.md, the context the loop iterates: read at run time (the
//! file in the source tree, so a learned rule applies to the very next
//! batch without a rebuild), its "Learned from listening" rules read and
//! replaced, and the generator's system text built from it.

use std::path::PathBuf;

/// The living guide in the source tree (`MUSIC_AGENTS_MD` overrides).
pub fn agents_path() -> PathBuf {
    match std::env::var_os("MUSIC_AGENTS_MD") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../score_live/src/guide/MUSIC-AGENTS.md"),
    }
}

/// The guide as it is now: the file, else the copy built in.
pub fn read() -> String {
    std::fs::read_to_string(agents_path()).unwrap_or_else(|_| makepad_score_live::guide::MUSIC_AGENTS.to_string())
}

const HEAD: &str = "## Learned from listening";
const TAIL: &str = "How to update this file:";

/// The section's rule lines (without their `- `).
pub fn learned(doc: &str) -> Vec<String> {
    let Some(start) = doc.find(HEAD) else { return Vec::new() };
    let body = &doc[start + HEAD.len()..];
    let end = body.find(TAIL).or_else(|| body.find("\n## ")).unwrap_or(body.len());
    body[..end]
        .lines()
        .filter_map(|l| l.trim().strip_prefix("- "))
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "(none yet)")
        .map(String::from)
        .collect()
}

/// `doc` with the section's rules replaced by `rules`.
pub fn with_learned(doc: &str, rules: &[String]) -> Option<String> {
    let start = doc.find(HEAD)? + HEAD.len();
    let body = &doc[start..];
    let end = start + body.find(TAIL).or_else(|| body.find("\n## "))?;
    let section = &doc[start..end];
    // Keep the section's intro paragraph (up to the first rule line).
    let intro_end = section.find("\n- ").map_or(section.trim_end().len(), |at| at + 1);
    let mut out = String::from(&doc[..start]);
    out.push_str(section[..intro_end].trim_end());
    out.push_str("\n\n");
    if rules.is_empty() {
        out.push_str("- (none yet)\n");
    }
    for rule in rules {
        out.push_str(&format!("- {}\n", rule.trim().trim_start_matches("- ")));
    }
    out.push('\n');
    out.push_str(&doc[end..]);
    Some(out)
}

/// Write the new rules into the guide; the text before, for the log.
pub fn write_learned(rules: &[String]) -> Result<String, String> {
    let path = agents_path();
    let before = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let after = with_learned(&before, rules).ok_or("MUSIC-AGENTS.md has no Learned from listening section")?;
    std::fs::write(&path, after).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(before)
}

/// The composer's system text: the Score guide with the guide as it is
/// now in place of the copy built in.
pub fn score_guide() -> String {
    let built = makepad_score_live::guide::guide();
    let base = built.strip_suffix(makepad_score_live::guide::MUSIC_AGENTS).unwrap_or(&built);
    format!("{base}{}", read())
}

/// The reference excerpts of imported covers (`midi_library refs`, a
/// development-only library): `references.tsv` and the excerpts beside it
/// in the weights folder, never in a checkout.
pub fn references_dir() -> PathBuf {
    std::env::var_os("MAKEPAD_ASSET_AI_CACHE")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| makepad_ai_hub::home::makepad_home().join("weights"))
        .join("datasets/lmd/references")
}

/// Words that say little about a style alone: one of them is not a match.
const GENERIC: [&str; 16] = ["pop", "rock", "electronic", "dance", "soul", "rnb", "jazz", "classic", "oldies", "favorites", "60s", "70s", "80s", "90s", "00s", "love"];

fn letters(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The reference a request matches best, from a `references.tsv`
/// (`slug<TAB>label<TAB>words<TAB>tempo and key<TAB>file`): its words found
/// in the request (one or two request words joined: "synth pop" is
/// "synthpop"), a specific style word counting double; at least two.
pub fn pick_reference(list: &str, request: &str) -> Option<(String, String)> {
    let words: Vec<String> = request.split(|c: char| !c.is_alphanumeric() && c != '\'').map(letters).filter(|w| !w.is_empty()).collect();
    let mut have: Vec<String> = words.clone();
    have.extend(words.windows(2).map(|w| format!("{}{}", w[0], w[1])));
    let mut best: Option<(u32, String, String)> = None;
    for line in list.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let (Some(label), Some(tags), Some(file)) = (fields.get(1), fields.get(2), fields.get(4)) else { continue };
        let score: u32 = tags.split(',').filter(|t| have.iter().any(|h| h == t)).map(|t| if GENERIC.contains(&t) { 1 } else { 2 }).sum();
        if score >= 2 && best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, label.to_string(), file.to_string()));
        }
    }
    best.map(|(_, label, file)| (label, file))
}

/// The excerpt to show a composer for this request, if one matches (none
/// when `MUSIC_REFERENCES=0`): one example, never pasted into every prompt.
pub fn reference_for(request: &str) -> Option<String> {
    if std::env::var("MUSIC_REFERENCES").is_ok_and(|v| v == "0") {
        return None;
    }
    let dir = references_dir();
    let list = std::fs::read_to_string(dir.join("references.tsv")).ok()?;
    let (label, file) = pick_reference(&list, request)?;
    let source = std::fs::read_to_string(dir.join(file)).ok()?;
    // A long excerpt is cut at a line; it is an example, not a score to run.
    let cut = if source.len() > 12_000 { &source[..source[..12_000].rfind('\n').unwrap_or(12_000)] } else { &source[..] };
    Some(format!(
        "A reference for this style: four bars of {label}, from its MIDI, as text (per bar the chords a beat at a time and each part on a sixteenth grid: a note starts, `~` holds, `.` rests; drums a row per drum, `x` hit). Study how its parts are voiced, layered and grooved (the drum pattern, the bass against the kick, the chord voicings and registers, the tune's range) and write your own song at that standard. Never copy its notes or melody.\n```text\n{cut}\n```\n"
    ))
}

/// The song library's tools for the director, as an MCP server: the
/// `midi_library` binary beside this one (or `MIDI_LIBRARY_BIN`) run as
/// `midi_library mcp`, when the library's index is on this machine (a
/// development machine; `MUSIC_REFERENCES=0` turns them off).
pub fn midi_tools() -> Option<(String, String)> {
    if std::env::var("MUSIC_REFERENCES").is_ok_and(|v| v == "0") {
        return None;
    }
    let bin = std::env::var_os("MIDI_LIBRARY_BIN").map(PathBuf::from).or_else(|| Some(std::env::current_exe().ok()?.with_file_name("midi_library")))?;
    if !bin.is_file() || !references_dir().with_file_name("index.tsv").is_file() {
        return None;
    }
    let command = bin.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    Some((format!(r#"{{"mcpServers":{{"midi":{{"command":"{command}","args":["mcp"]}}}}}}"#), "mcp__midi".to_string()))
}

/// Today as YYYY-MM-DD (UTC).
pub fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400) as i64;
    // Civil from days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_round_trip_through_the_section() {
        let doc = makepad_score_live::guide::MUSIC_AGENTS;
        let rules = vec!["2026-10-01 don't: detune a lead past 20 c; it sounded cheap.".to_string(), "2026-10-01 do: one bar of silence before the drop; it hit harder.".to_string()];
        let new = with_learned(doc, &rules).unwrap();
        assert_eq!(learned(&new), rules);
        assert!(new.contains(TAIL) && new.contains("## Units and conventions"));
        let cleared = with_learned(&new, &[]).unwrap();
        assert!(learned(&cleared).is_empty() && cleared.contains("- (none yet)"));
        assert_eq!(today().len(), 10);
    }

    #[test]
    fn a_reference_needs_a_matching_style() {
        let list = "a_ha_take_on_me\tA-ha - Take On Me (1985)\t1980s,80s,newwave,pop,synthpop\t169 bpm A major\ta_ha_take_on_me.txt\nnirvana_lithium\tNirvana - Lithium (1991)\tgrunge,rock\t123 bpm D major\tnirvana_lithium.txt\n";
        assert_eq!(pick_reference(list, "Style: 80s synth pop").map(|r| r.1), Some("a_ha_take_on_me.txt".to_string()));
        assert_eq!(pick_reference(list, "SUBGENRE: synthpop").map(|r| r.1), Some("a_ha_take_on_me.txt".to_string()));
        assert_eq!(pick_reference(list, "a grunge anthem").map(|r| r.1), Some("nirvana_lithium.txt".to_string()));
        assert_eq!(pick_reference(list, "upbeat pop for a launch video"), None);
        assert_eq!(pick_reference(list, "deep house"), None);
    }
}
