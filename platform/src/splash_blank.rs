//! Unused top-level Splash definitions, blanked for web builds.
//!
//! Every `script_mod!` block is embedded in the binary as its text. A web
//! build keeps only what its collect run used: a top-level definition no
//! run reached (`mod.widgets.ButtonIcon = ...`, a theme section, a `let`
//! template) is overwritten with spaces, every byte but newlines, so the
//! text keeps its length and lines and error locations in the rest stay
//! exact. [`top_statements`] finds the definitions in a block's text; a
//! blank list ([`BlankList`]) holds, per block, the text with its unused
//! definitions blanked. Natively, `MAKEPAD_SPLASH_BLANK=<file>` runs an
//! app on a blank list (each block evaluates its blanked text through the
//! live-reload overrides) so a build tool can check the result behaves
//! the same before it blanks the same ranges in the wasm.

use std::collections::HashMap;

pub use makepad_script_census::top_use::{top_statements, TopStatement};

/// Per `script_mod!` block (by file, line and column, as `ScriptModKey`),
/// its original text and its text with unused definitions blanked.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BlankList {
    pub blocks: Vec<BlankedBlock>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlankedBlock {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub original: String,
    pub blanked: String,
    /// The definitions blanked, by name (for reports).
    pub names: Vec<String>,
}

impl BlankList {
    /// Written as length-prefixed records: `file\tline\tcolumn\tnames\t
    /// <original len>\t<blanked len>\n`, then both texts.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for b in &self.blocks {
            out.extend_from_slice(format!("{}\t{}\t{}\t{}\t{}\t{}\n", b.file, b.line, b.column, b.names.join(","), b.original.len(), b.blanked.len()).as_bytes());
            out.extend_from_slice(b.original.as_bytes());
            out.extend_from_slice(b.blanked.as_bytes());
        }
        out
    }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut blocks = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            let nl = bytes[at..].iter().position(|&c| c == b'\n').ok_or("blank list: header without newline")? + at;
            let head = std::str::from_utf8(&bytes[at..nl]).map_err(|e| e.to_string())?;
            let f: Vec<&str> = head.split('\t').collect();
            if f.len() != 6 {
                return Err(format!("blank list: bad header `{head}`"));
            }
            let num = |s: &str| s.parse::<usize>().map_err(|e| format!("blank list: {e}"));
            let (ol, bl) = (num(f[4])?, num(f[5])?);
            let o = nl + 1;
            if o + ol + bl > bytes.len() {
                return Err("blank list: cut short".into());
            }
            let text = |r: std::ops::Range<usize>| String::from_utf8(bytes[r].to_vec()).map_err(|e| e.to_string());
            blocks.push(BlankedBlock {
                file: f[0].to_string(),
                line: num(f[1])?,
                column: num(f[2])?,
                names: f[3].split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                original: text(o..o + ol)?,
                blanked: text(o + ol..o + ol + bl)?,
            });
            at = o + ol + bl;
        }
        Ok(Self { blocks })
    }

    /// The pairs a wasm blanker writes (`makepad_web_pack::splash_blank`).
    pub fn pairs(&self) -> Vec<(String, String)> {
        self.blocks.iter().map(|b| (b.original.clone(), b.blanked.clone())).collect()
    }
}

/// The blank list of a finished run: in every `script_mod!` block, the
/// top-level definitions whose objects the heap no longer reaches from
/// what Rust holds ([`makepad_script_census::top_use`]).
pub fn compute_blank_list(vm: &mut crate::makepad_script::vm::ScriptVm) -> BlankList {
    let mut list = BlankList::default();
    for block in makepad_script_census::top_use::top_use(vm) {
        let unused: Vec<&TopStatement> = block.statements.iter().filter(|(_, used)| *used == Some(false)).map(|(s, _)| s).collect();
        if unused.is_empty() {
            continue;
        }
        let blanked = blank_statements(&block.code, &unused);
        list.blocks.push(BlankedBlock {
            file: block.file,
            line: block.line,
            column: block.column,
            names: unused.iter().map(|s| s.path.join(".")).collect(),
            original: block.code,
            blanked,
        });
    }
    list
}

impl BlankList {
    /// Bytes turned into spaces across all blocks.
    pub fn blanked_bytes(&self) -> usize {
        self.blocks.iter().map(|b| b.original.bytes().zip(b.blanked.bytes()).filter(|(a, c)| a != c).count()).sum()
    }

    /// The list without the definitions named in `keep` (`mod.widgets.X`
    /// paths or bare names), recomputed from each block's statements.
    pub fn without(&self, keep: &std::collections::HashSet<String>) -> BlankList {
        let mut out = BlankList::default();
        for b in &self.blocks {
            let statements = top_statements(&b.original);
            let blank: Vec<&TopStatement> = statements
                .iter()
                .filter(|s| b.names.contains(&s.path.join(".")))
                .filter(|s| !keep.contains(&s.path.join(".")) && !s.name().is_some_and(|n| keep.contains(n)))
                .collect();
            if blank.is_empty() {
                continue;
            }
            out.blocks.push(BlankedBlock {
                file: b.file.clone(),
                line: b.line,
                column: b.column,
                names: blank.iter().map(|s| s.path.join(".")).collect(),
                original: b.original.clone(),
                blanked: blank_statements(&b.original, &blank),
            });
        }
        out
    }
}

/// `code` with the ranges of the statements in `blank` turned into spaces
/// (newlines kept).
pub fn blank_statements(code: &str, blank: &[&TopStatement]) -> String {
    let mut bytes = code.as_bytes().to_vec();
    for s in blank {
        let mut i = s.start;
        while i < s.end {
            // A `#(n)` interpolation stays: the block's values are taken in
            // order, one per `#(..)`, so removing one shifts the rest.
            if bytes[i] == b'#' && bytes.get(i + 1) == Some(&b'(') {
                if let Some(close) = bytes[i..s.end].iter().position(|&c| c == b')') {
                    i += close + 1;
                    continue;
                }
            }
            if bytes[i] != b'\n' {
                bytes[i] = b' ';
            }
            i += 1;
        }
    }
    // The block's closing `;` (the macro ends every block with one) stays:
    // it ends the statement before a blanked tail.
    if code.ends_with(';') {
        *bytes.last_mut().unwrap() = b';';
    }
    String::from_utf8(bytes).unwrap_or_else(|_| code.to_string())
}

#[cfg(not(any(target_arch = "wasm32", target_os = "android", target_env = "ohos")))]
impl crate::Cx {
    /// `MAKEPAD_SPLASH_BLANK=<file>`: every block the list names evaluates
    /// its blanked text from here on (a build tool's check of a blank list
    /// before it blanks a wasm).
    pub(crate) fn init_splash_blank_from_env(&mut self) {
        let Ok(path) = std::env::var("MAKEPAD_SPLASH_BLANK") else { return };
        let list = match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| BlankList::read(&b)) {
            Ok(list) => list,
            Err(e) => {
                crate::error!("MAKEPAD_SPLASH_BLANK={path}: {e}");
                return;
            }
        };
        let mut overrides = self.script_data.live_reload.script_mod_overrides.borrow_mut();
        let mut by_key: HashMap<crate::makepad_script::vm::ScriptModKey, String> = HashMap::new();
        for b in list.blocks {
            by_key.insert(crate::makepad_script::vm::ScriptModKey { file: b.file, line: b.line, column: b.column }, b.blanked);
        }
        let n = by_key.len();
        overrides.extend(by_key);
        crate::log!("splash blank: {n} blocks run blanked ({path})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: &str = "use mod.prelude.widgets_internal.*\n    use mod.widgets.*\n\n    mod.widgets.ButtonBase = #(x)\n\n    /** flat */\n    mod.widgets.ButtonFlat = set_type_default() do mod.widgets.ButtonBase{\n        text: \"a { b\"\n        // a } comment\n        draw_text +: {\n            color: #fff\n        }\n    }\n\n    mod.widgets.Button = mod.widgets.ButtonFlat{\n        height: Fit\n    }\n    let Row = View{}\n        .with(1)\n;";

    #[test]
    fn finds_top_statements() {
        let s = top_statements(CODE);
        let names: Vec<Option<&str>> = s.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec![None, None, Some("ButtonBase"), Some("ButtonFlat"), Some("Button"), Some("Row")]);
        assert_eq!(s.first().unwrap().start, 0);
        assert_eq!(s.last().unwrap().end, CODE.len());
        for w in s.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        // The doc comment above ButtonFlat is its own.
        assert!(CODE[s[3].start..s[3].end].contains("/** flat */"));
        assert!(CODE[s[5].start..s[5].end].contains(".with(1)"));
    }

    #[test]
    fn blanking_keeps_lines_and_columns() {
        let s = top_statements(CODE);
        let out = blank_statements(CODE, &[&s[3]]);
        assert_eq!(out.len(), CODE.len());
        let lines = |t: &str| t.split('\n').map(|l| l.len()).collect::<Vec<_>>();
        assert_eq!(lines(&out), lines(CODE));
        // What follows the blanked definition sits at the same line and
        // column, so an error there is reported at the same place.
        let at = CODE.find("mod.widgets.Button =").unwrap();
        assert_eq!(&out[at..at + 20], "mod.widgets.Button =");
        let row_col = |t: &str, i: usize| (t[..i].matches('\n').count(), i - t[..i].rfind('\n').map_or(0, |p| p + 1));
        assert_eq!(row_col(&out, at), row_col(CODE, at));
    }

    #[test]
    fn blank_list_round_trips() {
        let list = BlankList {
            blocks: vec![BlankedBlock { file: "w/b.rs".into(), line: 3, column: 1, original: "ab\ncd".into(), blanked: "  \ncd".into(), names: vec!["X".into(), "Y".into()] }],
        };
        assert_eq!(BlankList::read(&list.write()).unwrap(), list);
    }
}
