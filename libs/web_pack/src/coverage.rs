//! What text a font face can show, decided from what a collect run saw
//! it draw, so a packer can cut the face to that and nothing a run could
//! still show goes missing.
//!
//! Inputs per face: the glyphs seen drawn, every row of text it laid out
//! and in how many frames, and what the app declared (coverage specs:
//! `all`, `latin`, `digits`, `U+0020-U+007E`, `chars:...`,
//! comma-separated). The classes:
//!
//! - **static**: only the glyphs seen drawn;
//! - **known sets**: plus every digit and number sign when it shows
//!   counters (texts that differ only in their digits), plus every
//!   character of a known corpus (e.g. a song's lyrics) when it shows
//!   words of it;
//! - **declared**: plus the declared ranges;
//! - **briefly shown**: texts shown for a frame or two that no counter,
//!   typing or corpus explains: Latin and its punctuation are kept as a
//!   margin (a packer then draws the app at random times with the cut
//!   fonts and fails on any glyph still missing);
//! - **open**: a declared `all` (a text input emits it): complete.
//!
//! When in doubt a face keeps more: a missing glyph at run time is a bug,
//! a few kilobytes are not.

use std::collections::{BTreeMap, BTreeSet};

/// Characters a counter may show between two analysed frames.
pub const NUMBER_CHARS: &str = "0123456789.,:%+-−";

/// Strings seen in this few frames count as changing every frame.
const TRANSIENT_FRAMES: u32 = 2;
/// More changing strings than this (not explained by digits, typing or
/// lyrics) make a face's text open.
const OPEN_STRINGS: usize = 3;

/// What a face can show.
#[derive(Clone, Debug, PartialEq)]
pub enum Coverage {
    /// Ship it complete, for this reason.
    Full(String),
    /// Cut it to these glyphs and characters, for these reasons.
    Subset { glyphs: BTreeSet<u16>, chars: BTreeSet<char>, why: Vec<String> },
}

/// The characters a coverage spec names (`all` is handled by the caller).
pub fn spec_chars(spec: &str) -> BTreeSet<char> {
    let mut out = BTreeSet::new();
    for part in spec.split(',').map(str::trim) {
        match part {
            "latin" => out.extend((0x20u32..=0x7e).chain(0xa0..=0xff).filter_map(char::from_u32)),
            "digits" => out.extend(NUMBER_CHARS.chars()),
            p if p.starts_with("chars:") => out.extend(p["chars:".len()..].chars()),
            p => {
                if let Some((a, b)) = p.strip_prefix("U+").and_then(|r| r.split_once('-')) {
                    if let (Ok(a), Ok(b)) = (u32::from_str_radix(a, 16), u32::from_str_radix(b.trim_start_matches("U+"), 16)) {
                        out.extend((a..=b.min(a + 0x10000)).filter_map(char::from_u32));
                    }
                }
            }
        }
    }
    out
}

pub fn same_family(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect::<String>();
    norm(a) == norm(b)
}

/// Decides one face's coverage from what the analysis saw of it.
pub fn classify(seen: &BTreeSet<u16>, texts: &[(String, u32)], declared: &[&str], lyrics: &str) -> Coverage {
    if declared.iter().any(|s| s.split(',').any(|p| p.trim() == "all")) {
        return Coverage::Full("a document declares `glyphs: @all`".into());
    }
    let mut chars = BTreeSet::new();
    let mut why = Vec::new();
    if !seen.is_empty() {
        why.push(format!("{} glyphs seen drawn in {} texts", seen.len(), texts.len()));
    }
    // Counters: texts that differ only in their digits.
    let template = |t: &str| t.chars().map(|c| if c.is_ascii_digit() { '#' } else { c }).collect::<String>();
    let mut templates: BTreeMap<String, usize> = BTreeMap::new();
    for (t, _) in texts {
        *templates.entry(template(t)).or_default() += 1;
    }
    let counters = templates.iter().filter(|(t, n)| t.contains('#') && **n > 1).count();
    let digits = texts.iter().any(|(t, _)| t.chars().any(|c| c.is_ascii_digit()));
    if counters > 0 || digits {
        chars.extend(NUMBER_CHARS.chars());
        why.push(if counters > 0 { format!("{counters} counters: every digit") } else { "digits shown: every digit".into() });
    }
    // Lyric words.
    let is_lyric = |t: &str| t.trim().chars().count() >= 2 && lyrics.contains(t.trim());
    if !lyrics.is_empty() && texts.iter().any(|(t, _)| is_lyric(t)) {
        chars.extend(lyrics.chars().filter(|c| !c.is_control()));
        why.push("shows lyric words: every character of the lyrics".into());
    }
    // Open text: strings shown in a frame or two that are not explained
    // by a counter, a typewriter (a prefix of a longer text) or the lyrics.
    let all: Vec<&str> = texts.iter().map(|(t, _)| t.as_str()).collect();
    let open: Vec<&str> = texts
        .iter()
        .filter(|(t, frames)| {
            *frames <= TRANSIENT_FRAMES
                && templates.get(&template(t)).copied().unwrap_or(0) <= 1
                && !is_lyric(t)
                && !all.iter().any(|o| o.len() > t.len() && (o.starts_with(t.as_str()) || o.ends_with(t.as_str())))
        })
        .map(|(t, _)| t.as_str())
        .collect();
    // Explicit declarations bound text; a number's own `digits` does not
    // speak for other texts of the face.
    let bounding: Vec<&str> = declared.iter().copied().filter(|s| s.trim() != "digits").collect();
    let declared_chars: BTreeSet<char> = declared.iter().flat_map(|s| spec_chars(s)).collect();
    if open.len() > OPEN_STRINGS && bounding.is_empty() {
        // Texts shown for a frame or two: static labels on a cut, or text
        // made per frame. Keep Latin and its punctuation as a margin (the
        // packer's glyph check at random times catches anything beyond).
        let example: String = open[0].chars().take(24).collect();
        chars.extend(spec_chars("latin"));
        why.push(format!("{} texts shown for a frame or two (e.g. \"{example}\"): Latin and punctuation kept as a margin", open.len()));
    } else if open.len() > OPEN_STRINGS {
        why.push(format!("{} briefly shown texts, bounded by the declared coverage", open.len()));
    }
    if !declared_chars.is_empty() {
        why.push(format!("declared: {}", declared.join(", ")));
        chars.extend(declared_chars);
    }
    Coverage::Subset { glyphs: seen.clone(), chars, why }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn texts(t: &[(&str, u32)]) -> Vec<(String, u32)> {
        t.iter().map(|(s, n)| (s.to_string(), *n)).collect()
    }

    #[test]
    fn static_text_is_cut_to_what_was_drawn() {
        let c = classify(&[5, 6].into(), &texts(&[("HELLO", 300)]), &[], "");
        assert!(matches!(c, Coverage::Subset { ref chars, .. } if chars.is_empty()));
    }

    #[test]
    fn counters_keep_every_digit() {
        let c = classify(&BTreeSet::new(), &texts(&[("P(DOOM) 12%", 1), ("P(DOOM) 13%", 1), ("P(DOOM) 14%", 1), ("P(DOOM) 15%", 1), ("P(DOOM) 16%", 1)]), &[], "");
        let Coverage::Subset { chars, .. } = c else { panic!("{c:?}") };
        assert!(('0'..='9').all(|d| chars.contains(&d)));
    }

    #[test]
    fn typing_and_lyrics_are_bounded() {
        let c = classify(&BTreeSet::new(), &texts(&[("I", 1), ("I'm", 1), ("I'm up", 1), ("I'm upping", 1), ("I'm upping my", 9)]), &[], "");
        assert!(matches!(c, Coverage::Subset { .. }));
        let lyrics = "I'm upping my P(doom)\nwake up";
        let c = classify(&BTreeSet::new(), &texts(&[("upping", 1), ("wake", 1), ("my", 1), ("doom", 1), ("up", 1)]), &[], lyrics);
        let Coverage::Subset { chars, .. } = c else { panic!() };
        assert!(chars.contains(&'w') && chars.contains(&'P'));
    }

    #[test]
    fn open_text_ships_complete_unless_declared() {
        let scramble = texts(&[("xQ#v", 1), ("pLmz", 1), ("Kd!e", 1), ("aaZt", 1), ("RRqo", 1)]);
        let Coverage::Subset { chars, .. } = classify(&BTreeSet::new(), &scramble, &[], "") else { panic!() };
        assert!(chars.contains(&'Z') && chars.contains(&'~'));
        let Coverage::Subset { chars, .. } = classify(&BTreeSet::new(), &scramble, &["latin"], "") else { panic!() };
        assert!(chars.contains(&'Z') && chars.contains(&'é'));
        assert!(matches!(classify(&BTreeSet::new(), &texts(&[("A", 9)]), &["all"], ""), Coverage::Full(_)));
    }

    #[test]
    fn ranges_parse() {
        assert_eq!(spec_chars("U+0041-U+0043").into_iter().collect::<String>(), "ABC");
        assert_eq!(spec_chars("chars:xy, U+0041-0041").into_iter().collect::<String>(), "Axy");
    }
}
