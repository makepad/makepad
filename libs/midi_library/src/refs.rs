//! Reference excerpts: a few bars of an imported song (the first chorus,
//! or the fullest section) kept as text ([`crate::roll`]) in the library's
//! `references/` folder, listed in `references.tsv` with the words that
//! describe it (genre, tags, decade, tempo, key). The music AI is shown at
//! most one, and only when a request's style matches it
//! (`makepad_music_iter::context::reference_for`), as an example of
//! how a real arrangement is voiced, layered and grooved. Never copied,
//! not part of Stage: the folder lives beside the downloads, outside every
//! checkout.

use crate::dataset;
use crate::form::Section;
use crate::import::{self, About, Options};
use crate::roll::{self, Detail};
use crate::index::Entry;
use crate::meta::norm;
use std::path::PathBuf;

/// Bars an excerpt runs.
pub const BARS: usize = 4;

pub fn dir() -> PathBuf {
    dataset::dir().join("references")
}

pub fn list_path() -> PathBuf {
    dir().join("references.tsv")
}

/// The section an excerpt is taken from: the first chorus, else the
/// fullest section of four bars or more.
pub fn pick_section(sections: &[Section]) -> Option<&Section> {
    sections
        .iter()
        .find(|s| s.kind == "chorus")
        .or_else(|| sections.iter().filter(|s| s.to - s.from >= BARS).max_by(|a, b| a.energy.total_cmp(&b.energy)))
        .or(sections.first())
}

/// The words a reference is found by.
pub fn words(entry: &Entry) -> Vec<String> {
    let mut out: Vec<String> = entry.genres.iter().map(|g| norm(g)).collect();
    out.extend(entry.tags.iter().take(10).map(|(t, _)| t.clone()));
    if let Some(d) = entry.decade() {
        out.push(format!("{}s", d % 100));
        out.push(format!("{d}s"));
    }
    out.sort();
    out.dedup();
    out
}

/// Write the excerpt of `entry` (its MIDI `bytes`) to the references
/// folder as text ([`crate::roll`]: the header, then the bars of its first
/// chorus) and list it; the file's path.
pub fn write(entry: &Entry, bytes: &[u8], about: &About) -> Result<PathBuf, String> {
    let full = import::import(bytes, about, &Options::default())?;
    let section = pick_section(&full.sections).ok_or("no sections")?;
    let from = section.from;
    let to = (from + BARS).min(section.to.max(from + 1));
    let detail = Detail { bars: Some((from + 1, to)), collapse: false, ..Detail::default() };
    let mut text = format!("REFERENCE: bars {}-{} ({}) of {}. Learn how its parts are voiced, layered and grooved; never copy its notes.\n", from + 1, to, section.name, entry.label());
    text.push_str(&roll::to_text(&full.arrangement, &detail));
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let path = dir().join(format!("{}.txt", entry.slug()));
    std::fs::write(&path, &text).map_err(|e| format!("{}: {e}", path.display()))?;
    // The list: one line per reference, replaced when written again.
    let list = std::fs::read_to_string(list_path()).unwrap_or_default();
    let mut lines: Vec<String> = list.lines().filter(|l| !l.is_empty() && !l.starts_with(&format!("{}\t", entry.slug()))).map(str::to_string).collect();
    lines.push(format!("{}\t{}\t{}\t{} bpm {}\t{}", entry.slug(), entry.label().replace('\t', " "), words(entry).join(","), entry.features.bpm, entry.features.key, path.file_name().unwrap().to_string_lossy()));
    lines.sort();
    std::fs::write(list_path(), lines.join("\n") + "\n").map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_chorus_else_the_fullest() {
        let s = |name: &str, kind: &str, from: usize, to: usize, energy: f64| Section { name: name.into(), kind: kind.into(), from, to, letter: 'A', energy };
        let a = vec![s("intro", "intro", 0, 4, 1.0), s("verse_1", "verse", 4, 12, 3.0), s("chorus_1", "chorus", 12, 20, 5.0)];
        assert_eq!(pick_section(&a).unwrap().name, "chorus_1");
        let b = vec![s("intro", "intro", 0, 4, 1.0), s("bridge", "bridge", 4, 12, 4.0), s("outro", "outro", 12, 14, 6.0)];
        assert_eq!(pick_section(&b).unwrap().name, "bridge");
    }
}
