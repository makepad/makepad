//! What a preset's file name says about it.
//!
//! MilkDrop never stored an author or a title inside the preset: the file
//! name is the only record, written by hand over twenty years. The common
//! shape is `Author - Title`, with several authors joined by `+`, `&`, `,`
//! or ` n ` (`Flexi + Martin - tunnel`), a title that may itself contain
//! ` - ` (a mashup names its parts), an editor's pass appended as
//! `--- Isosceles edit`, and whole presets chained with ` === `. Plenty of
//! names follow none of it (`208`, `Mig_177`). The reading here is tolerant:
//! it only ever splits where the convention is unambiguous, and otherwise
//! keeps the whole stem as the title.

/// A file stem read as title, authors and editors.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetName {
    /// The name as it reads best in a list: the stem without its authors
    /// and without trailing edit notes.
    pub title: String,
    /// Who wrote it, in order; empty when the name does not say.
    pub authors: Vec<String>,
    /// Who edited it afterwards (`--- Isosceles edit`).
    pub editors: Vec<String>,
}

/// Author parts longer than this are a title that happens to contain
/// ` - `, not a list of names.
const MAX_AUTHOR_CHARS: usize = 48;

/// Read `stem` (a file name without `.milk`).
pub fn parse_name(stem: &str) -> PresetName {
    let stem = stem.trim();
    let mut editors = Vec::new();
    let mut body = stem;
    // `... --- Isosceles edit05`: an editor's note, possibly repeated.
    while let Some((before, note)) = body.rsplit_once("---") {
        match editor_of(note) {
            Some(editor) => {
                editors.insert(0, editor);
                body = before.trim_end();
            }
            None => break,
        }
    }
    // `A === B` chains two presets (a transition): its " - " pieces are not
    // one author and one title, so the whole chain is the title.
    let chained = body.contains(" === ");
    let (authors, title) = match body.split_once(" - ") {
        Some((head, rest)) if !chained && is_author_list(head) && !rest.trim().is_empty() => (split_authors(head), rest.trim()),
        _ => (Vec::new(), body),
    };
    PresetName { title: title.to_string(), authors, editors }
}

/// The editor named by a `--- Name edit` note, if that is what it is.
fn editor_of(note: &str) -> Option<String> {
    let note = note.trim();
    let lower = note.to_lowercase();
    let at = lower.rfind(" edit")?;
    // `edit`, `edit05`, `edit b`: the word is the note's last one or two.
    let after = &lower[at + 5..];
    if after.split_whitespace().count() > 1 {
        return None;
    }
    let name = note[..at].trim();
    (!name.is_empty() && name.chars().count() <= MAX_AUTHOR_CHARS).then(|| name.to_string())
}

/// Whether the text before the first ` - ` reads as names rather than as
/// the start of a title: short, no brackets, and every name in it at most
/// three words (`midgitstraights of majillaen`, not `Adam Favorite
/// Milkdrop Master Mashup FX 2 martin`).
fn is_author_list(head: &str) -> bool {
    let head = head.trim();
    !head.is_empty()
        && head.chars().count() <= MAX_AUTHOR_CHARS
        && !head.contains('(')
        && !head.contains('[')
        && split_authors(head).iter().all(|name| name.split_whitespace().count() <= 3)
}

/// `Flexi, Martin + Orb & Geiss n Zylot` → each name once, in order.
fn split_authors(head: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for part in head.split(['+', '&', ',']) {
        for name in part.split(" n ").flat_map(|p| p.split(" and ")) {
            let name = name.trim();
            if !name.is_empty() && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name.to_string());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(stem: &str) -> (String, Vec<&'static str>, Vec<&'static str>) {
        let name = parse_name(stem);
        let leak = |v: Vec<String>| v.into_iter().map(|s| &*Box::leak(s.into_boxed_str())).collect();
        (name.title, leak(name.authors), leak(name.editors))
    }

    /// Names taken from the "cream of the crop" pack as they are written.
    #[test]
    fn real_world_names() {
        assert_eq!(read("Flexi - crush ice 75"), ("crush ice 75".into(), vec!["Flexi"], vec![]));
        assert_eq!(read("Flexi + Martin - tunnel of supraschismatika"), ("tunnel of supraschismatika".into(), vec!["Flexi", "Martin"], vec![]));
        assert_eq!(read("Stahlregen & Geiss + ORB - Etheral Waves (MashUp 20 - All-Spark RMX) sensual moral beavers").1, vec!["Stahlregen", "Geiss", "ORB"]);
        assert_eq!(
            read("hexcollie, ti@n, suksma, zylot n orb - blarffff fire --- Isosceles edit"),
            ("blarffff fire".into(), vec!["hexcollie", "ti@n", "suksma", "zylot", "orb"], vec!["Isosceles"])
        );
        assert_eq!(read("suksma - truth, rarity --- Isosceles edit05"), ("truth, rarity".into(), vec!["suksma"], vec!["Isosceles"]));
        assert_eq!(
            read("TonyMilkdrop - Angels Of Glory [Flexi - try again + techstyle] --- Isosceles edit"),
            ("Angels Of Glory [Flexi - try again + techstyle]".into(), vec!["TonyMilkdrop"], vec!["Isosceles"])
        );
        // A mashup keeps its inner " - " in the title.
        assert_eq!(read("suksma - never degweh - flx spr col").0, "never degweh - flx spr col");
        // No convention at all: the stem is the title.
        assert_eq!(read("208"), ("208".into(), vec![], vec![]));
        assert_eq!(read("Mig_177"), ("Mig_177".into(), vec![], vec![]));
        assert_eq!(read("Serge + 158 001"), ("Serge + 158 001".into(), vec![], vec![]));
        // A long first part is a title, not an author list.
        assert_eq!(
            read("Adam Favorite Milkdrop Master Mashup FX 2 martin - disco mix + Lodus + Geiss + Ludicrous speed + Eos Ft").1,
            Vec::<&str>::new()
        );
        // A transition chain is one title; only its editor is split off.
        assert_eq!(
            read("Fast transition to black - levels effect === Goody's Lightning (ps 2-0) --- Isosceles edit"),
            ("Fast transition to black - levels effect === Goody's Lightning (ps 2-0)".into(), vec![], vec!["Isosceles"])
        );
        // "edit" inside a title is not an editor note.
        assert_eq!(read("shifter - spincycle (sweeet candy) Studio Music tweak").2, Vec::<&str>::new());
        assert_eq!(read("A - b --- this is not an edit note at all").2, Vec::<&str>::new());
    }
}
