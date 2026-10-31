//! The cheat sheet (SUPER+/): every binding of `binds::keymap`, grouped. It
//! is generated from that one table, so it cannot drift from what the keys
//! do. Families that differ only by a digit are shown as one row.
use crate::binds::{combo_text, keymap, Bind, WmAction};
use makepad_widgets::KeyCode;

/// The headings, in the order the sheet shows them.
pub const GROUPS: [&str; 6] = ["Spaces and shelves", "Layout and focus", "Windows", "Groups", "Shell", "Glasses"];

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub combo: String,
    pub help: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

fn is_digit(k: KeyCode) -> bool {
    use KeyCode::*;
    matches!(k, Key0 | Key1 | Key2 | Key3 | Key4 | Key5 | Key6 | Key7 | Key8 | Key9)
}

/// `Super+1` -> `Super+1..9`.
fn digit_family(combo: &str, last: char) -> String {
    let base = combo.split("  (").next().unwrap_or(combo);
    let head = base.trim_end_matches(|c: char| c.is_ascii_digit());
    format!("{head}1..{last}")
}

pub fn sections() -> Vec<Section> {
    sections_of(&keymap())
}

pub fn sections_of(binds: &[Bind]) -> Vec<Section> {
    let mut out: Vec<Section> = GROUPS.iter().map(|t| Section { title: t, rows: Vec::new() }).collect();
    let mut families: Vec<(&'static str, String)> = Vec::new();
    for b in binds {
        let title = b.action.group();
        let Some(sec) = out.iter_mut().find(|s| s.title == title) else { continue };
        // One row for a family of digit chords.
        let family = is_digit(b.key)
            && matches!(
                b.action,
                WmAction::SpaceJump(_) | WmAction::MoveToSpace(_) | WmAction::MoveToSpaceSilent(_) | WmAction::GroupActive(_)
            );
        if family {
            let key = format!("{:?}{}", b.layer, b.help);
            if families.iter().any(|(t, k)| *t == title && *k == key) {
                continue;
            }
            families.push((title, key));
            let last = match b.action {
                WmAction::GroupActive(_) => '5',
                _ => '9',
            };
            sec.rows.push(Row { combo: digit_family(&combo_text(b), last), help: b.help.replace(" N", " n") });
            continue;
        }
        let combo = combo_text(b);
        if sec.rows.iter().any(|r| r.combo == combo && r.help == b.help) {
            continue;
        }
        sec.rows.push(Row { combo, help: b.help.to_string() });
    }
    out.retain(|s| !s.rows.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_rows() -> Vec<Row> {
        sections().into_iter().flat_map(|s| s.rows).collect()
    }

    #[test]
    fn every_distinct_binding_is_on_the_sheet() {
        let rows = all_rows();
        let helps: Vec<&str> = rows.iter().map(|r| r.help.as_str()).collect();
        for b in keymap() {
            if is_digit(b.key) && matches!(b.action, WmAction::SpaceJump(_) | WmAction::MoveToSpace(_) | WmAction::MoveToSpaceSilent(_) | WmAction::GroupActive(_)) {
                continue;
            }
            assert!(helps.contains(&b.help), "missing: {}", b.help);
        }
    }

    #[test]
    fn digit_chords_collapse_to_one_row() {
        let rows = all_rows();
        let jumps: Vec<&Row> = rows.iter().filter(|r| r.help == "Slide to space n").collect();
        assert_eq!(jumps.len(), 1);
        assert!(jumps[0].combo.contains("1..9"), "{}", jumps[0].combo);
        assert!(!rows.iter().any(|r| r.help == "Slide to space N"));
    }

    #[test]
    fn the_space_keys_are_there_under_their_heading() {
        let secs = sections();
        let spaces = secs.iter().find(|s| s.title == "Spaces and shelves").unwrap();
        for want in ["Slide to the previous space", "Slide to the next space", "Show or hide the space strip", "Full screen (a space of its own)"] {
            assert!(spaces.rows.iter().any(|r| r.help == want), "{want}");
        }
        assert!(secs.iter().flat_map(|s| &s.rows).any(|r| r.help.starts_with("Cheat sheet")));
        assert!(secs.iter().all(|s| !s.rows.is_empty()));
    }
}
