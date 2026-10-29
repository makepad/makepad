//! The Code view's folds: every block's code or lens at once, and single
//! blocks overriding that until the next fold-all. Saved per score (by its
//! label) in the data folder; the source is never touched.

use makepad_score_live::source;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folds {
    key: String,
    pub code_all: bool,
    pub lens_all: bool,
    /// node -> (code folded, lens folded) overrides.
    blocks: BTreeMap<String, (Option<bool>, Option<bool>)>,
}

impl Folds {
    fn path(key: &str) -> std::path::PathBuf {
        crate::paths::data_dir().join("folds").join(format!("{}.txt", source::fnv_hex(key)))
    }

    /// The key folds are kept under for a score labelled `label`.
    pub fn key_of(label: &str) -> String {
        if label.is_empty() { "untitled".to_string() } else { label.to_string() }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// The folds saved for the score labelled `key`.
    pub fn load(key: &str) -> Folds {
        let mut folds = Folds { key: key.to_string(), ..Folds::default() };
        let Ok(text) = std::fs::read_to_string(Self::path(key)) else { return folds };
        let flag = |v: &str| match v {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        };
        for line in text.lines() {
            match line.split_whitespace().collect::<Vec<_>>().as_slice() {
                ["code_all", v] => folds.code_all = *v == "1",
                ["lens_all", v] => folds.lens_all = *v == "1",
                ["block", node, code, lens] => {
                    folds.blocks.insert(node.to_string(), (flag(code), flag(lens)));
                }
                _ => {}
            }
        }
        folds
    }

    fn save(&self) {
        let flag = |v: Option<bool>| match v {
            Some(true) => "1",
            Some(false) => "0",
            None => "-",
        };
        let mut text = format!("code_all {}\nlens_all {}\n", self.code_all as u8, self.lens_all as u8);
        for (node, (code, lens)) in &self.blocks {
            text.push_str(&format!("block {node} {} {}\n", flag(*code), flag(*lens)));
        }
        let path = Self::path(&self.key);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    /// The block's code is folded (its first line and its lens show).
    pub fn code(&self, node: &str) -> bool {
        self.blocks.get(node).and_then(|b| b.0).unwrap_or(self.code_all)
    }

    /// The block's lens is folded (a small chip after its first line).
    pub fn lens(&self, node: &str) -> bool {
        self.blocks.get(node).and_then(|b| b.1).unwrap_or(self.lens_all)
    }

    /// A fold-all: every block follows it again.
    pub fn set_all(&mut self, code: bool, on: bool) {
        if code {
            self.code_all = on;
        } else {
            self.lens_all = on;
        }
        for b in self.blocks.values_mut() {
            if code {
                b.0 = None;
            } else {
                b.1 = None;
            }
        }
        self.blocks.retain(|_, b| b.0.is_some() || b.1.is_some());
        self.save();
    }

    /// One block's toggle.
    pub fn toggle(&mut self, node: &str, code: bool) {
        let now = if code { self.code(node) } else { self.lens(node) };
        let entry = self.blocks.entry(node.to_string()).or_default();
        if code {
            entry.0 = Some(!now);
        } else {
            entry.1 = Some(!now);
        }
        self.save();
    }
}

/// The 0-based lines the Code view folds for `source`: the lines after a
/// folded block's first (its `let` line stays, with its lens under it).
pub fn folded_lines(source: &str, outline: &source::Outline, folds: &Folds) -> Vec<usize> {
    let mut out = Vec::new();
    for node in &outline.nodes {
        if !folds.code(&node.name) {
            continue;
        }
        let first = source::line_col(source, node.span.start).0 - 1;
        let last = source::line_col(source, node.span.end.saturating_sub(1)).0 - 1;
        out.extend(first + 1..=last);
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folded_block_keeps_its_first_line() {
        let src = "let a = Synth{\n    cutoff: 800\n}\nlet b = Loop{play: a}\nScore{a b}\n";
        let outline = source::outline(src);
        let mut folds = Folds::default();
        folds.blocks.insert("a".into(), (Some(true), None));
        assert_eq!(folded_lines(src, &outline, &folds), vec![1, 2]);
        folds.code_all = true;
        folds.blocks.clear();
        // A one-line block has nothing to fold.
        assert_eq!(folded_lines(src, &outline, &folds), vec![1, 2]);
    }
}
