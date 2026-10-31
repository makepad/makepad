//! Timed lyrics for AI songs: when each line (and word) of the writer's
//! lyrics is sung, and its translation.
//!
//! The timing comes from the generation itself. YuE2 writes a score before
//! it sings and sings it at the score's tempo; the hub returns that score's
//! timeline (`SongTimingJson`: its sections with start and end, and the sung
//! notes in each). Here each lyric section is matched to a score section by
//! name and order ([`time_from_score`]), and its lines are spread over that
//! section's sung notes by syllable count (one line per phrase when the
//! phrases and the lines agree). Without a score (other models, `score_mode`
//! off) the times are estimated from the section structure and the length
//! ([`estimate`]), and say so.
//!
//! Stored as enhanced LRC ([`to_lrc`]): `[mm:ss.xx]<mm:ss.xx>word <mm:ss.xx>word …<mm:ss.xx>`
//! per line (the last stamp is where the line ends), a section's tag as its
//! own line (`[mm:ss.xx][Verse]`). The translation is kept as the writer
//! wrote it (the same sections and lines) and paired line by line.

use makepad_ai_hub::protocol::SongTimingJson;

/// Where the times came from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Timing {
    /// The model's own score.
    Score,
    /// The section structure and the length.
    Estimated,
}

impl Timing {
    pub fn name(self) -> &'static str {
        match self {
            Timing::Score => "score",
            Timing::Estimated => "estimated",
        }
    }

    pub fn parse(name: &str) -> Option<Timing> {
        match name {
            "score" => Some(Timing::Score),
            "estimated" => Some(Timing::Estimated),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    /// A space follows it (not between CJK characters).
    pub space: bool,
    pub start: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub words: Vec<Word>,
    pub start: f64,
    pub end: f64,
    /// The translation under it (empty: none).
    pub translation: String,
}

impl Line {
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (n, word) in self.words.iter().enumerate() {
            out.push_str(&word.text);
            if word.space && n + 1 < self.words.len() {
                out.push(' ');
            }
        }
        out
    }
}

/// A section as shown: its tag ("Verse"; empty before the first tag), when
/// it starts, and its lines (none for an instrumental one).
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub name: String,
    pub start: f64,
    pub lines: Vec<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimedLyrics {
    pub sections: Vec<Section>,
    pub timing: Timing,
}

impl TimedLyrics {
    /// Lines in order, with their section index.
    pub fn lines(&self) -> impl Iterator<Item = (usize, &Line)> {
        self.sections.iter().enumerate().flat_map(|(s, section)| section.lines.iter().map(move |l| (s, l)))
    }

    /// The line sung at `t` (the last one started), and how far into its
    /// words: the index of the last word started.
    pub fn at(&self, t: f64) -> Option<(usize, usize)> {
        let mut found = None;
        for (n, (_, line)) in self.lines().enumerate() {
            if line.start <= t {
                let word = line.words.iter().rposition(|w| w.start <= t).unwrap_or(0);
                found = Some((n, word));
            } else {
                break;
            }
        }
        found
    }
}

// ---- the writer's text -----------------------------------------------------

/// Lyrics as sections: `[Tag]` lines start one; text before the first tag
/// is a section with no name.
pub fn sections(lyrics: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for raw in lyrics.lines() {
        let line = raw.trim();
        if line.len() >= 2 && line.starts_with('[') && line.ends_with(']') {
            out.push((line[1..line.len() - 1].trim().to_string(), Vec::new()));
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push((String::new(), Vec::new()));
        }
        out.last_mut().unwrap().1.push(line.to_string());
    }
    out
}

/// The translation's lines for each lyric line: by section and line when
/// the writer kept the structure, else by line order when the counts agree,
/// else none.
fn pair_translation(lyrics: &[(String, Vec<String>)], translation: &str) -> Vec<Vec<String>> {
    let none: Vec<Vec<String>> = lyrics.iter().map(|(_, lines)| vec![String::new(); lines.len()]).collect();
    let trans = sections(translation);
    let trans: Vec<&(String, Vec<String>)> = trans.iter().filter(|(_, l)| !l.is_empty()).collect();
    let sung: Vec<&(String, Vec<String>)> = lyrics.iter().filter(|(_, l)| !l.is_empty()).collect();
    if trans.is_empty() {
        return none;
    }
    let flat: Vec<String> = if trans.len() == sung.len() && trans.iter().zip(&sung).all(|(t, s)| t.1.len() == s.1.len()) {
        trans.iter().flat_map(|t| t.1.iter().cloned()).collect()
    } else {
        let all: Vec<String> = trans.iter().flat_map(|t| t.1.iter().cloned()).collect();
        if all.len() != sung.iter().map(|s| s.1.len()).sum::<usize>() {
            return none;
        }
        all
    };
    let mut flat = flat.into_iter();
    lyrics.iter().map(|(_, lines)| lines.iter().map(|_| flat.next().unwrap_or_default()).collect()).collect()
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xac00..=0xd7af | 0xf900..=0xfaff)
}

/// A line's words: whitespace-separated, CJK characters one each; each
/// with whether a space follows it.
pub fn tokens(line: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for token in line.split_whitespace() {
        if token.chars().any(is_cjk) {
            let mut current = String::new();
            for c in token.chars() {
                if is_cjk(c) && !current.is_empty() {
                    out.push((std::mem::take(&mut current), false));
                }
                current.push(c);
            }
            if !current.is_empty() {
                out.push((current, true));
            }
            if let Some(last) = out.last_mut() {
                last.1 = true;
            }
        } else {
            out.push((token.to_string(), true));
        }
    }
    out
}

/// Sung syllables of a word: a CJK character is one; otherwise vowel
/// groups (a silent final "e" not counted), at least one.
pub fn syllables(word: &str) -> usize {
    let cjk = word.chars().filter(|c| is_cjk(*c)).count();
    if cjk > 0 {
        return cjk;
    }
    let lower: Vec<char> = word.to_lowercase().chars().filter(|c| c.is_alphabetic()).collect();
    let vowel = |c: char| "aeiouyàáâäãåæèéêëìíîïòóôöõøœùúûüý".contains(c);
    let mut count = 0;
    let mut previous = false;
    for &c in &lower {
        let v = vowel(c);
        if v && !previous {
            count += 1;
        }
        previous = v;
    }
    let n = lower.len();
    if count > 1 && n > 2 && lower[n - 1] == 'e' && !vowel(lower[n - 2]) && !(lower[n - 2] == 'l' && !vowel(lower[n - 3])) {
        count -= 1;
    }
    count.max(1)
}

// ---- what YuE2 sings ------------------------------------------------------

/// The section tags YuE2 was trained with.
const YUE2_TAGS: [&str; 7] = ["Intro", "Verse", "Pre-Chorus", "Chorus", "Interlude", "Bridge", "Outro"];

/// Words of a line that only directs the band ("Guitar solo", "(sax
/// break)"): instruments and performance words. A line made only of these
/// is a stage direction, not words to sing.
const DIRECTION_WORDS: &[&str] = &[
    "solo", "instrumental", "break", "breakdown", "riff", "fill", "fills", "plays", "playing", "drop", "build", "buildup", "fade", "fades", "out", "in", "intro", "outro", "interlude",
    "guitar", "guitars", "bass", "drums", "drum", "piano", "keys", "synth", "synths", "sax", "saxophone", "trumpet", "trumpets", "horns", "horn", "brass", "strings", "violin", "cello",
    "flute", "organ", "accordion", "harp", "bells", "choir", "pads", "beat", "the", "a", "and", "with", "kicks", "kick",
];

/// The writer's lyrics as YuE2 should get them: only its section tags on
/// their own lines (numbered or decorated variants become the plain tag; a
/// "[Guitar Solo]" or "[Instrumental]" becomes an empty [Interlude]; any
/// other bracketed line goes), no directions inside a line ("(sax)",
/// "*whispered*", "[x2]"), and no line that only directs the band. Every
/// word left is sung.
pub fn clean_for_yue2(lyrics: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for raw in lyrics.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            let inner = line[1..line.len() - 1].to_lowercase();
            let key = norm(&inner);
            let tag = YUE2_TAGS.iter().find(|t| key.starts_with(&norm(t)));
            let tag = match tag {
                Some(tag) => Some(*tag),
                None if ["solo", "instrumental", "break"].iter().any(|w| inner.contains(w)) => Some("Interlude"),
                None => None,
            };
            if let Some(tag) = tag {
                if !out.is_empty() && !out.last().is_some_and(|l| l.is_empty()) {
                    out.push(String::new());
                }
                out.push(format!("[{tag}]"));
            }
            continue;
        }
        // Directions inside the line: (..), [..], {..}, *..*.
        let mut kept = String::new();
        let mut close: Option<char> = None;
        for c in line.chars() {
            match close {
                Some(end) if c == end => close = None,
                Some(_) => {}
                None => match c {
                    '(' => close = Some(')'),
                    '[' => close = Some(']'),
                    '{' => close = Some('}'),
                    '*' => close = Some('*'),
                    _ => kept.push(c),
                },
            }
        }
        let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
        let words: Vec<String> = kept.split_whitespace().map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()).filter(|w| !w.is_empty()).collect();
        if words.is_empty() || words.iter().all(|w| DIRECTION_WORDS.contains(&w.as_str())) {
            continue;
        }
        out.push(kept);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

// ---- timing ---------------------------------------------------------------

fn norm(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Lines spread over notes by syllable count: line `l` gets its words'
/// start times. `notes` are (onset, length).
fn spread(lines: &[Vec<(String, bool)>], notes: &[(f64, f64)]) -> Vec<(Vec<f64>, f64, f64)> {
    let counts: Vec<Vec<usize>> = lines.iter().map(|l| l.iter().map(|(w, _)| syllables(w)).collect()).collect();
    let total: usize = counts.iter().flatten().sum::<usize>().max(1);
    let n = notes.len();
    // Syllable k of `total` starts at a point along the notes.
    let time_of = |k: f64| -> f64 {
        let x = k * n as f64 / total as f64;
        let i = (x.floor() as usize).min(n - 1);
        let f = if n >= total { 0.0 } else { x - i as f64 };
        notes[i].0 + f * notes[i].1
    };
    let end_of = |k: f64| -> f64 {
        let x = k * n as f64 / total as f64;
        let i = (x.floor() as usize).min(n - 1);
        notes[i].0 + notes[i].1
    };
    let mut at = 0usize;
    let mut out = Vec::new();
    for line in &counts {
        let mut starts = Vec::new();
        let first = at;
        for &c in line {
            starts.push(time_of(at as f64));
            at += c;
        }
        let start = starts.first().copied().unwrap_or_else(|| time_of(first as f64));
        let end = if at > first { end_of((at - 1) as f64) } else { start };
        out.push((starts, start, end));
    }
    out
}

/// Phrases: runs of notes with gaps under 0.3 s between them.
fn phrases(notes: &[(f64, f64)]) -> Vec<&[(f64, f64)]> {
    let mut out = Vec::new();
    let mut from = 0;
    for i in 1..notes.len() {
        if notes[i].0 - (notes[i - 1].0 + notes[i - 1].1) >= 0.3 {
            out.push(&notes[from..i]);
            from = i;
        }
    }
    if from < notes.len() {
        out.push(&notes[from..]);
    }
    out
}

fn timed_line(words: &[(String, bool)], starts: &[f64], start: f64, end: f64, translation: String) -> Line {
    Line {
        words: words.iter().zip(starts).map(|((text, space), &start)| Word { text: text.clone(), space: *space, start }).collect(),
        start,
        end: end.max(start),
        translation,
    }
}

/// Lyrics timed by the model's score. `None` when the score has no sung
/// notes (then [`estimate`]).
pub fn time_from_score(lyrics: &str, translation: &str, timing: &SongTimingJson, length: f64) -> Option<TimedLyrics> {
    let parsed = sections(lyrics);
    let trans = pair_translation(&parsed, translation);
    let sung: Vec<usize> = (0..parsed.len()).filter(|&i| !parsed[i].1.is_empty()).collect();
    let voiced: Vec<usize> = (0..timing.sections.len()).filter(|&j| !timing.sections[j].notes.is_empty()).collect();
    if sung.is_empty() || voiced.is_empty() {
        return None;
    }
    // Align the sung lyric sections with the voiced score sections by name
    // (an edit distance: a lyric section without a match joins the one
    // before it; a voiced score section without one is sung without words).
    let (n, m) = (sung.len(), voiced.len());
    let name_a: Vec<String> = sung.iter().map(|&i| norm(&parsed[i].0)).collect();
    let name_b: Vec<String> = voiced.iter().map(|&j| norm(&timing.sections[j].name)).collect();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in 0..=n {
        for j in 0..=m {
            dp[i][j] = match (i, j) {
                (0, _) => j as u32,
                (_, 0) => i as u32,
                _ => {
                    let cost = if name_a[i - 1] == name_b[j - 1] { 0 } else { 2 };
                    (dp[i - 1][j - 1] + cost).min(dp[i - 1][j] + 1).min(dp[i][j - 1] + 1)
                }
            };
        }
    }
    let mut matched: Vec<Option<usize>> = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        let cost = if name_a[i - 1] == name_b[j - 1] { 0 } else { 2 };
        if dp[i][j] == dp[i - 1][j - 1] + cost {
            matched[i - 1] = Some(j - 1);
            i -= 1;
            j -= 1;
        } else if dp[i][j] == dp[i - 1][j] + 1 {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    // Unmatched lyric sections join the match before them (or after, at the start).
    let first = matched.iter().flatten().next().copied().unwrap_or(0);
    let mut last = first;
    let groups: Vec<usize> = matched
        .iter()
        .map(|m| {
            if let Some(m) = m {
                last = *m;
            }
            last
        })
        .collect();
    // Each voiced score section's lines, spread over its notes.
    let mut line_times: Vec<Vec<(Vec<f64>, f64, f64)>> = vec![Vec::new(); n];
    for b in 0..m {
        let members: Vec<usize> = (0..n).filter(|&a| groups[a] == b).collect();
        if members.is_empty() {
            continue;
        }
        let lines: Vec<Vec<(String, bool)>> = members.iter().flat_map(|&a| parsed[sung[a]].1.iter().map(|l| tokens(l))).collect();
        let notes: Vec<(f64, f64)> = timing.sections[voiced[b]].notes.iter().map(|n| (n.start, n.length)).collect();
        let runs = phrases(&notes);
        let times: Vec<(Vec<f64>, f64, f64)> = if runs.len() == lines.len() {
            lines.iter().zip(&runs).flat_map(|(line, run)| spread(std::slice::from_ref(line), run)).collect()
        } else {
            spread(&lines, &notes)
        };
        let mut times = times.into_iter();
        for &a in &members {
            line_times[a] = parsed[sung[a]].1.iter().map(|_| times.next().unwrap_or_default()).collect();
        }
    }
    // The sections, with instrumental ones placed at their score section.
    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut previous_end = 0.0f64;
    for (s, (name, lines)) in parsed.iter().enumerate() {
        if let Some(a) = sung.iter().position(|&i| i == s) {
            let times = &line_times[a];
            let timed: Vec<Line> = lines
                .iter()
                .zip(times)
                .zip(&trans[s])
                .map(|((text, (starts, start, end)), t)| timed_line(&tokens(text), starts, start.min(length), end.min(length), t.clone()))
                .collect();
            cursor = voiced[groups[a]] + 1;
            let start = timed.first().map_or(previous_end, |l| l.start);
            previous_end = timed.last().map_or(previous_end, |l| l.end);
            out.push(Section { name: name.clone(), start, lines: timed });
        } else {
            let found = (cursor..timing.sections.len()).find(|&j| norm(&timing.sections[j].name) == norm(name));
            let start = match found {
                Some(j) => {
                    cursor = j + 1;
                    timing.sections[j].start
                }
                None => previous_end,
            };
            out.push(Section { name: name.clone(), start: start.min(length), lines: Vec::new() });
        }
    }
    Some(TimedLyrics { sections: out, timing: Timing::Score })
}

/// Lyrics timed from their structure alone: an instrumental section takes
/// as long as three short lines, a sung line by its syllables, with a short
/// breath between lines; all scaled to `length`.
pub fn estimate(lyrics: &str, translation: &str, length: f64) -> TimedLyrics {
    let parsed = sections(lyrics);
    let trans = pair_translation(&parsed, translation);
    const INSTRUMENTAL: f64 = 18.0;
    const BREATH: f64 = 1.5;
    let line_units = |l: &str| tokens(l).iter().map(|(w, _)| syllables(w)).sum::<usize>().max(4) as f64;
    let total: f64 = parsed
        .iter()
        .map(|(_, lines)| if lines.is_empty() { INSTRUMENTAL } else { lines.iter().map(|l| line_units(l) + BREATH).sum::<f64>() + BREATH })
        .sum::<f64>()
        .max(1.0);
    let unit = length.max(1.0) / total;
    let mut t = 0.0;
    let mut out = Vec::new();
    for (s, (name, lines)) in parsed.iter().enumerate() {
        let start = t;
        if lines.is_empty() {
            t += INSTRUMENTAL * unit;
            out.push(Section { name: name.clone(), start, lines: Vec::new() });
            continue;
        }
        let mut timed = Vec::new();
        for (text, translation) in lines.iter().zip(&trans[s]) {
            let words = tokens(text);
            let units = line_units(text);
            let sung: usize = words.iter().map(|(w, _)| syllables(w)).sum::<usize>().max(1);
            let mut at = 0usize;
            let starts: Vec<f64> = words
                .iter()
                .map(|(w, _)| {
                    let s = t + at as f64 / sung as f64 * units * unit;
                    at += syllables(w);
                    s
                })
                .collect();
            timed.push(timed_line(&words, &starts, t, t + units * unit, translation.clone()));
            t += (units + BREATH) * unit;
        }
        t += BREATH * unit;
        out.push(Section { name: name.clone(), start, lines: timed });
    }
    TimedLyrics { sections: out, timing: Timing::Estimated }
}

// ---- storage ---------------------------------------------------------------

fn stamp(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{:02}:{:02}.{:02}", cs / 6000, (cs / 100) % 60, cs % 100)
}

fn parse_stamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    Some(m.trim().parse::<f64>().ok()? * 60.0 + rest.trim().parse::<f64>().ok()?)
}

/// Enhanced LRC: a line per section tag and per lyric line, word stamps,
/// the line's end as the last stamp.
pub fn to_lrc(lyrics: &TimedLyrics) -> String {
    let mut out = String::new();
    for section in &lyrics.sections {
        if !section.name.is_empty() {
            out.push_str(&format!("[{}][{}]\n", stamp(section.start), section.name));
        }
        for line in &section.lines {
            out.push_str(&format!("[{}]", stamp(line.start)));
            for (n, word) in line.words.iter().enumerate() {
                out.push_str(&format!("<{}>{}", stamp(word.start), word.text));
                if word.space && n + 1 < line.words.len() {
                    out.push(' ');
                }
            }
            out.push_str(&format!("<{}>\n", stamp(line.end)));
        }
    }
    out
}

/// Back from [`to_lrc`], with the translation paired again.
pub fn from_lrc(lrc: &str, translation: &str, timing: Timing) -> TimedLyrics {
    let mut out: Vec<Section> = Vec::new();
    for raw in lrc.lines() {
        let Some(rest) = raw.strip_prefix('[') else { continue };
        let Some((at, body)) = rest.split_once(']') else { continue };
        let Some(start) = parse_stamp(at) else { continue };
        let body = body.trim_end();
        if body.starts_with('[') && body.ends_with(']') {
            out.push(Section { name: body[1..body.len() - 1].to_string(), start, lines: Vec::new() });
            continue;
        }
        let mut words = Vec::new();
        let mut end = start;
        for part in body.split('<').filter(|p| !p.is_empty()) {
            let Some((at, text)) = part.split_once('>') else { continue };
            let Some(t) = parse_stamp(at) else { continue };
            if text.trim().is_empty() {
                end = t;
                continue;
            }
            words.push(Word { text: text.trim().to_string(), space: text.ends_with(' '), start: t });
            end = t;
        }
        if words.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push(Section { name: String::new(), start, lines: Vec::new() });
        }
        if let Some(last) = words.last_mut() {
            last.space = true;
        }
        out.last_mut().unwrap().lines.push(Line { words, start, end, translation: String::new() });
    }
    let structure: Vec<(String, Vec<String>)> = out.iter().map(|s| (s.name.clone(), s.lines.iter().map(|l| l.text()).collect())).collect();
    let trans = pair_translation(&structure, translation);
    for (section, trans) in out.iter_mut().zip(trans) {
        for (line, t) in section.lines.iter_mut().zip(trans) {
            line.translation = t;
        }
    }
    TimedLyrics { sections: out, timing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_hub::protocol::{SongNoteJson, SongSectionJson};

    const LYRICS: &str = "[Intro]\n\n[Verse]\nSnow is falling on the city lights\nEvery window glowing gold tonight\n\n[Chorus]\nChristmas lights on the dance floor\nFeel the beat, we want more\n\n[Outro]\n";
    const DUTCH: &str = "[Intro]\n\n[Verse]\nSneeuw valt op de lichten van de stad\nElk raam gloeit vanavond goud\n\n[Chorus]\nKerstlichten op de dansvloer\nVoel de beat, we willen meer\n\n[Outro]\n";

    fn section(name: &str, start: f64, end: f64, notes: &[(f64, f64)]) -> SongSectionJson {
        SongSectionJson { name: name.into(), start, end, notes: notes.iter().map(|&(start, length)| SongNoteJson { start, length }).collect() }
    }

    /// Nine notes a phrase, a 1 s rest between phrases.
    fn phrase(at: f64, count: usize) -> Vec<(f64, f64)> {
        (0..count).map(|k| (at + k as f64 * 0.4, 0.35)).collect()
    }

    #[test]
    fn syllables_are_counted() {
        assert_eq!(syllables("Snow"), 1);
        assert_eq!(syllables("falling"), 2);
        assert_eq!(syllables("tonight"), 2);
        assert_eq!(syllables("time"), 1);
        assert_eq!(syllables("candle"), 2);
        assert_eq!(syllables("beat,"), 1);
        assert_eq!(syllables("路灯"), 2);
        assert_eq!(tokens("路灯眨着 眼睛").len(), 6);
    }

    #[test]
    fn lines_follow_the_score_phrases() {
        let mut verse = phrase(10.0, 9);
        verse.extend(phrase(15.0, 9));
        let mut chorus = phrase(22.0, 8);
        chorus.extend(phrase(27.0, 6));
        let timing = SongTimingJson {
            seconds: 40.0,
            sections: vec![section("intro", 0.0, 9.0, &[]), section("verse", 9.0, 21.0, &verse), section("chorus", 21.0, 34.0, &chorus), section("outro", 34.0, 40.0, &[])],
            ..SongTimingJson::default()
        };
        let timed = time_from_score(LYRICS, DUTCH, &timing, 40.0).unwrap();
        assert_eq!(timed.timing, Timing::Score);
        let names: Vec<&str> = timed.sections.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Intro", "Verse", "Chorus", "Outro"]);
        assert_eq!(timed.sections[0].start, 0.0);
        assert_eq!(timed.sections[3].start, 34.0);
        let lines: Vec<&Line> = timed.lines().map(|(_, l)| l).collect();
        // One line a phrase: each starts on its phrase's first note.
        assert_eq!(lines.iter().map(|l| l.start).collect::<Vec<_>>(), [10.0, 15.0, 22.0, 27.0]);
        assert!((lines[0].end - (10.0 + 8.0 * 0.4 + 0.35)).abs() < 1e-9);
        // "Snow is fall-ing": the third word on the third note.
        assert!((lines[0].words[2].start - 10.8).abs() < 1e-9);
        assert_eq!(lines[1].translation, "Elk raam gloeit vanavond goud");
        assert_eq!(timed.at(16.2), Some((1, 1)));
        assert_eq!(timed.at(5.0), None);
    }

    #[test]
    fn an_unmatched_section_joins_the_one_before() {
        // The model sang verse and pre-chorus as one "verse" and an extra
        // instrumental chorus.
        let lyrics = "[Verse]\na b c d\ne f g h\n\n[Pre-Chorus]\ni j k l\n\n[Chorus]\nm n o p\n";
        let verse: Vec<(f64, f64)> = (0..12).map(|k| (k as f64 * 0.5, 0.4)).collect();
        let chorus: Vec<(f64, f64)> = (0..4).map(|k| (20.0 + k as f64 * 0.5, 0.4)).collect();
        let timing = SongTimingJson {
            seconds: 30.0,
            sections: vec![section("verse", 0.0, 10.0, &verse), section("chorus", 10.0, 18.0, &[]), section("chorus", 18.0, 30.0, &chorus)],
            ..SongTimingJson::default()
        };
        let timed = time_from_score(lyrics, "", &timing, 30.0).unwrap();
        let starts: Vec<f64> = timed.lines().map(|(_, l)| l.start).collect();
        assert_eq!(starts, [0.0, 2.0, 4.0, 20.0]);
        assert!(timed.lines().all(|(_, l)| l.translation.is_empty()));
    }

    #[test]
    fn estimate_fills_the_length_in_order() {
        let timed = estimate(LYRICS, "", 60.0);
        assert_eq!(timed.timing, Timing::Estimated);
        let lines: Vec<&Line> = timed.lines().map(|(_, l)| l).collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].start > 0.0, "the intro comes first");
        assert!(lines.windows(2).all(|w| w[0].end <= w[1].start));
        assert!(lines[3].end < 60.0 && timed.sections[3].start < 60.0);
    }

    #[test]
    fn lrc_round_trips() {
        let timed = estimate(LYRICS, DUTCH, 60.0);
        let lrc = to_lrc(&timed);
        assert!(lrc.starts_with("[00:00.00][Intro]\n"));
        assert!(lrc.contains("<") && lrc.contains("Snow "));
        let back = from_lrc(&lrc, DUTCH, Timing::Estimated);
        assert_eq!(back.sections.len(), timed.sections.len());
        for ((_, a), (_, b)) in back.lines().zip(timed.lines()) {
            assert_eq!(a.text(), b.text());
            assert_eq!(a.translation, b.translation);
            assert!((a.start - b.start).abs() < 0.006 && (a.end - b.end).abs() < 0.006);
            assert_eq!(a.words.len(), b.words.len());
        }
        let cjk = estimate("[Verse]\n路灯眨着眼睛 偷看谁的身影\n", "", 20.0);
        let back = from_lrc(&to_lrc(&cjk), "", Timing::Estimated);
        assert_eq!(back.lines().next().unwrap().1.text(), "路灯眨着眼睛 偷看谁的身影");
    }

    #[test]
    fn a_translation_that_lost_the_structure_is_paired_by_line_or_dropped() {
        let parsed = sections(LYRICS);
        let by_line = pair_translation(&parsed, "one\ntwo\nthree\nfour");
        assert_eq!(by_line[1], ["one", "two"]);
        let dropped = pair_translation(&parsed, "only one line");
        assert!(dropped.iter().flatten().all(|t| t.is_empty()));
    }

    #[test]
    fn yue2_gets_only_its_tags_and_sung_words() {
        let raw = "[Intro]\n(soft piano)\n\n[Verse 1]\nSnow is falling (falling) on the town\n*whispered* Every window glowing\nGuitar solo\n\n[Sax Solo]\n\n[Chorus - big]\nFeel the beat tonight [x2]\nBells go ring and bass go boom\n[Drop]\n[Pre-Chorus]\nHold on\n";
        let clean = clean_for_yue2(raw);
        assert_eq!(
            clean,
            "[Intro]\n\n[Verse]\nSnow is falling on the town\nEvery window glowing\n\n[Interlude]\n\n[Chorus]\nFeel the beat tonight\nBells go ring and bass go boom\n\n[Pre-Chorus]\nHold on\n"
        );
        assert_eq!(clean_for_yue2(&clean), clean);
    }
}
