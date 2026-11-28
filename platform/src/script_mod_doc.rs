//! Structural edits on a Splash document or one editable body of a Rust file.
use crate::live_reload::{scan_script_mods, skip_non_code_segment, ScriptModBlock};
use makepad_script::syntax::{
    self, edit::Replace, literal_class::parse_literal_class,
    widget::{find_object_by_open_brace, set_widget_prop, WidgetEdit}, write::ColorStyle,
    At, Kind, Loc, Syntax,
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind { RustScriptMod, Splash }

#[derive(Clone, Debug)]
pub struct ScriptModDoc {
    pub kind: DocKind,
    pub path: String,
    pub text: String,
    pub blocks: Vec<ScriptModBlock>,
}

#[derive(Clone, Debug)]
pub struct DocEditResult {
    pub next_text: String,
    pub file_edits: Vec<Replace>,
    pub focus_open_byte: Option<usize>,
    pub focus_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustRefSite {
    pub line: usize,
    pub snippet: String,
}

// Keep the exact trivia and omit the runtime scanner's synthetic terminator.
struct Body {
    text: String,
    file: Range<usize>,
    placeholders: Vec<(Range<usize>, Range<usize>)>, // editable, file
}

impl Body {
    fn file_offset(&self, byte: usize) -> Result<usize, String> {
        if !self.text.is_char_boundary(byte) { return Err("invalid block edit boundary".into()); }
        let n = self.placeholders.partition_point(|(p, _)| p.end <= byte);
        if self.placeholders.get(n).is_some_and(|(p, _)| p.start < byte) {
            return Err("edit boundary inside a Rust placeholder".into());
        }
        let (local, file) = n.checked_sub(1).map_or((0, self.file.start), |i| {
            let (p, f) = &self.placeholders[i];
            (p.end, f.end)
        });
        Ok(file + (byte - local))
    }
}

impl ScriptModDoc {
    pub fn parse(path: impl Into<String>, text: String, kind: DocKind) -> Result<Self, String> {
        let path = path.into();
        let blocks = match kind {
            DocKind::RustScriptMod => scan_script_mods(&path, &text)?,
            DocKind::Splash => { syntax::parse(&text).map_err(|e| e.to_string())?; Vec::new() }
        };
        Ok(Self { kind, path, text, blocks })
    }

    fn body(&self, index: usize) -> Result<Body, String> {
        if self.kind == DocKind::Splash && index == 0 {
            return Ok(Body { file: 0..self.text.len(), text: self.text.clone(), placeholders: vec![] });
        }
        let block = self.blocks.get(index).ok_or_else(|| format!("no script block {index}"))?;
        let mut text = String::new();
        let mut placeholders = Vec::new();
        let mut cursor = block.body_start;
        for (k, file) in block.placeholders.iter().enumerate() {
            text.push_str(self.text.get(cursor..file.start).ok_or("invalid placeholder range")?);
            let start = text.len();
            text.push_str(&format!("#({k})"));
            placeholders.push((start..text.len(), file.clone()));
            cursor = file.end;
        }
        text.push_str(self.text.get(cursor..block.body_end).ok_or("invalid block range")?);
        Ok(Body { text, file: block.body_start..block.body_end, placeholders })
    }

    pub fn block_syntax(&self, block_index: usize) -> Result<Syntax, String> {
        syntax::parse(&self.body(block_index)?.text).map_err(|e| e.to_string())
    }

    pub fn block_offset_from_file(&self, block_index: usize, file_byte: usize) -> Result<usize, String> {
        let body = self.body(block_index)?;
        if !(body.file.start..=body.file.end).contains(&file_byte) || !self.text.is_char_boundary(file_byte) {
            return Err("invalid file offset for this block".into());
        }
        let n = body.placeholders.partition_point(|(_, file)| file.end <= file_byte);
        if body.placeholders.get(n).is_some_and(|(_, file)| file.start < file_byte) {
            return Err("file offset inside a Rust placeholder".into());
        }
        let (block, file) = n.checked_sub(1).map_or((0, body.file.start), |i| {
            let (block, file) = &body.placeholders[i];
            (block.end, file.end)
        });
        Ok(block + (file_byte - file))
    }

    pub fn file_offset_from_block(&self, block_index: usize, block_byte: usize) -> Result<usize, String> {
        self.body(block_index)?.file_offset(block_byte)
    }

    pub fn hash(&self) -> u64 {
        self.text.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100000001b3))
    }

    pub fn apply_block_edit(&self, block_index: usize, edit: WidgetEdit) -> Result<DocEditResult, String> {
        let body = self.body(block_index)?;
        let mut edits = edit.edits;
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        let mut file_edits = Vec::with_capacity(edits.len());
        let mut next_text = String::with_capacity(self.text.len());
        let mut cursor = 0;
        for e in &edits {
            let start = body.file_offset(e.range.start)?;
            let end = body.file_offset(e.range.end)?;
            if start < cursor || end < start { return Err("reversed or overlapping block edits".into()); }
            if self.kind == DocKind::RustScriptMod {
                let i = body.placeholders.partition_point(|(p, _)| p.end <= e.range.start);
                if body.placeholders.get(i).is_some_and(|(p, _)| p.start < e.range.end)
                    || e.text.contains("#(") || e.text.contains("@(") {
                    return Err("a structural edit cannot change a Rust placeholder".into());
                }
            }
            next_text.push_str(&self.text[cursor..start]);
            next_text.push_str(&e.text);
            file_edits.push(Replace::new(start..end, &e.text));
            cursor = end;
        }
        next_text.push_str(&self.text[cursor..]);
        let next = Self::parse(self.path.clone(), next_text, self.kind)?;
        if next.blocks.len() != self.blocks.len() { return Err("edit changed script block count".into()); }
        for (old, new) in self.blocks.iter().zip(&next.blocks) {
            if old.placeholders.len() != new.placeholders.len() || old.placeholders.iter().zip(&new.placeholders)
                .any(|(a,b)| self.text[a.clone()] != next.text[b.clone()]) {
                return Err("edit changed the Rust placeholders".into());
            }
        }
        let next_body = next.body(block_index)?;
        let parsed = syntax::parse(&next_body.text).map_err(|e| e.to_string())?;
        if parsed != edit.syntax { return Err("file edits do not reproduce the checked block syntax".into()); }
        let focus_open_byte = edit.focus_open_brace.map(|byte| {
            find_object_by_open_brace(&parsed, byte).ok_or("focus does not identify an object")?;
            next_body.file_offset(byte)
        }).transpose()?;
        Ok(DocEditResult { next_text: next.text, file_edits, focus_open_byte, focus_name: edit.focus_name })
    }

    pub fn apply_theme_tokens(&self, block_index: usize, base: Option<&str>, sheet: Option<&str>, tokens: &[(String, String)]) -> Result<DocEditResult, String> {
        let syntax = self.block_syntax(block_index)?;
        let colors = match self.kind { DocKind::RustScriptMod => ColorStyle::MacroHashX, DocKind::Splash => ColorStyle::Hash };
        if base.is_some_and(|name| !identifier(name)) { return Err("invalid theme base identifier".into()); }
        for (name, value) in tokens {
            if !identifier(name) { return Err(format!("invalid theme token name `{name}`")); }
            parse_literal_class(value)?;
        }
        let named = base.map(|id| format!("mod.themes.{id}"));
        let mut override_loc = None;
        let mut named_loc = None;
        let mut assigned = false;
        for (i, stmt) in syntax.statements().iter().enumerate() {
            match &stmt.kind {
                Kind::Entry { name, op, value, .. } if name == "mod.theme" && op == "+:" && value.object().is_some() => {
                    override_loc = Some(Loc(vec![i, 0]));
                }
                Kind::Assign { target, op, value, .. } if op == "=" => {
                    if named.is_some() && target.ident() == named.as_deref() && value.object().is_some() {
                        named_loc = Some(Loc(vec![i, 1]));
                    }
                    assigned |= target.ident() == Some("mod.theme") && value.ident().is_some_and(|p| p.starts_with("mod.themes."));
                }
                _ => {}
            }
        }
        let edit = if let Some(loc) = named_loc.or(override_loc) {
            theme_fields(syntax, &loc, tokens, colors)?
        } else {
            let seed = syntax::parse("mod.theme +: {\n}").map_err(|e| e.to_string())?;
            let block = theme_fields(seed, &Loc(vec![0, 0]), tokens, colors)?;
            let mut statements = Vec::new();
            if let Some(base) = named.filter(|_| !assigned) { statements.push(format!("mod.theme = {base}")); }
            let mut text = String::new();
            if let Some(sheet) = sheet.filter(|s| !s.is_empty()) {
                for line in sheet.lines() { text.push_str("// theme sheet: "); text.push_str(line); text.push('\n'); }
            }
            text.push_str(block.syntax.source());
            statements.push(text);
            insert_theme(syntax, &statements, self.kind)?
        };
        self.apply_block_edit(block_index, edit)
    }

    /// References in the surrounding Rust source, with one-based source lines.
    pub fn find_rust_id_refs(&self, name: &str) -> Vec<RustRefSite> {
        if self.kind != DocKind::RustScriptMod || !identifier(name) { return Vec::new(); }
        let mut sites = Vec::new();
        let mut start = 0;
        for block in &self.blocks {
            rust_refs(&self.text, start..block.body_start, name, &mut sites);
            start = block.body_end;
        }
        rust_refs(&self.text, start..self.text.len(), name, &mut sites);
        sites
    }
}

fn identifier(text: &str) -> bool {
    let mut bytes = text.bytes();
    bytes.next().is_some_and(|b| b == b'_' || b.is_ascii_alphabetic())
        && bytes.all(|b| b == b'_' || b.is_ascii_alphanumeric())
}

// Compose consecutive AST edits in original coordinates. Original pieces
// survive as ranges, even when intervening source contains a Rust placeholder.
enum Piece { Original(Range<usize>), Added(String) }
impl Piece {
    fn len(&self) -> usize {
        match self { Self::Original(range) => range.len(), Self::Added(s) => s.len() }
    }
    fn split(&mut self, at: usize) -> Piece {
        match self {
            Self::Original(range) => {
                let end = range.end;
                range.end = range.start + at;
                Self::Original(range.end..end)
            }
            Self::Added(s) => Self::Added(s.split_off(at)),
        }
    }
}

struct Draft {
    syntax: Syntax,
    pieces: Vec<Piece>,
    original_len: usize,
}
impl Draft {
    fn new(syntax: Syntax) -> Self {
        let original_len = syntax.source().len();
        Self { syntax, original_len, pieces: vec![Piece::Original(0..original_len)] }
    }
    fn boundary(&mut self, byte: usize) -> usize {
        let mut offset = 0;
        for (i, piece) in self.pieces.iter_mut().enumerate() {
            if offset == byte { return i; }
            if byte < offset + piece.len() {
                let right = piece.split(byte - offset);
                self.pieces.insert(i + 1, right);
                return i + 1;
            }
            offset += piece.len();
        }
        self.pieces.len()
    }
    fn advance(&mut self, next: Syntax, mut edits: Vec<Replace>) {
        edits.sort_by_key(|e| (e.range.start, e.range.end));
        for edit in edits.into_iter().rev() {
            let start = self.boundary(edit.range.start);
            let end = self.boundary(edit.range.end);
            self.pieces.splice(start..end, Some(Piece::Added(edit.text)));
        }
        self.syntax = next;
    }
    fn finish(self, loc: &Loc) -> WidgetEdit {
        let mut edits = Vec::new();
        let mut start = 0;
        let mut added = String::new();
        for piece in self.pieces.into_iter().chain(Some(Piece::Original(self.original_len..self.original_len))) {
            match piece {
                Piece::Added(text) => added.push_str(&text),
                Piece::Original(range) => {
                    if start < range.start || !added.is_empty() {
                        edits.push(Replace::new(start..range.start, std::mem::take(&mut added)));
                    }
                    start = range.end;
                }
            }
        }
        let focus_open_brace = self.syntax.node(loc).and_then(|n| match n.kind {
            Kind::Object { open, .. } => Some(open), _ => None,
        });
        WidgetEdit { syntax: self.syntax, edits, focus_open_brace, focus_name: None }
    }
}

fn theme_fields(syntax: Syntax, loc: &Loc, tokens: &[(String, String)], colors: ColorStyle) -> Result<WidgetEdit, String> {
    let mut draft = Draft::new(syntax);
    for (name, value) in tokens {
        let edit = set_widget_prop(&draft.syntax, loc, name, value, colors).map_err(|e| e.to_string())?;
        draft.advance(edit.syntax, edit.edits);
    }
    Ok(draft.finish(loc))
}

fn insert_theme(syntax: Syntax, statements: &[String], kind: DocKind) -> Result<WidgetEdit, String> {
    let mut at = syntax.statements().iter().take_while(|s| matches!(s.kind, Kind::Use { .. })).count();
    let mut draft = Draft::new(syntax);
    for statement in statements {
        let position = if at == 0 { At::AfterHeader } else { At::After(at - 1) };
        let (checked, edits) = draft.syntax.insert_indented_root_statement(position, statement, kind == DocKind::RustScriptMod)
            .map_err(|e| e.to_string())?;
        draft.advance(checked.syntax, edits);
        at += 1;
    }
    Ok(draft.finish(&Loc(vec![at - 1, 0])))
}

fn rust_refs(src: &str, range: Range<usize>, name: &str, out: &mut Vec<RustRefSite>) {
    let mut tokens = Vec::new();
    let mut i = range.start;
    while i < range.end {
        if let Ok(Some(end)) = skip_non_code_segment(src.as_bytes(), i) {
            if !src[i..].starts_with("//") && !src[i..].starts_with("/*") { tokens.push((i, "")); }
            i = end;
            continue;
        }
        let c = src[i..].chars().next().unwrap();
        let start = i;
        i += c.len_utf8();
        if c.is_whitespace() { continue; }
        if c == '_' || c.is_alphanumeric() {
            for c in src[i..range.end].chars() {
                if c != '_' && !c.is_alphanumeric() { break; }
                i += c.len_utf8();
            }
        }
        tokens.push((start, &src[start..i]));
    }
    for (i, &(at, word)) in tokens.iter().enumerate() {
        if !matches!(word, "id" | "ids") || tokens.get(i + 1).map(|t| t.1) != Some("!")
            || tokens.get(i + 2).map(|t| t.1) != Some("(") { continue; }
        let args = &tokens[i + 3..];
        let found = if word == "id" {
            args.first().is_some_and(|t| t.1 == name) && args.get(1).is_some_and(|t| t.1 == ")")
        } else {
            args.iter().take_while(|t| t.1 != ")").any(|t| t.1 == name)
        };
        if found {
            let start = src[..at].rfind('\n').map_or(0, |j| j + 1);
            let end = src[at..].find('\n').map_or(src.len(), |j| at + j);
            out.push(RustRefSite { line: 1 + src[..start].bytes().filter(|&b| b == b'\n').count(), snippet: src[start..end].trim().to_owned() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_script::syntax::widget::{delete_widget, duplicate_widget, rename_widget};

    #[test]
    fn script_mod_doc_refuses_placeholder_delete_or_duplicate() {
        let src = "script_mod!{ View{value: #(first)} }\nfn f() {\n    let _ = id!(save); // ids!(save)\n    let _ = ids!(panel.save);\n    let _ = \"id!(save)\";\n}\nscript_mod!{\n    // café\n    View {\n        protected := Button{text: #(my_rust_val)}\n        // keep this\n        save := Label{}\n    }\n}\n";
        let doc = ScriptModDoc::parse("app.rs", src.into(), DocKind::RustScriptMod).unwrap();
        let tree = doc.block_syntax(1).unwrap();
        let protected = Loc(vec![0, 0]);
        for edit in [delete_widget(&tree, &protected).unwrap(), duplicate_widget(&tree, &protected, None).unwrap()] {
            assert!(doc.apply_block_edit(1, edit).unwrap_err().contains("placeholder"), "deleting or copying a Rust placeholder is refused");
            assert_eq!(doc.text, src, "failed edits preserve every file byte");
        }
        let edit = rename_widget(&tree, &Loc(vec![0, 1]), "save_button").unwrap();
        let out = doc.apply_block_edit(1, edit).unwrap();
        let expected = src.replace("save :=", "save_button :=");
        assert_eq!(out.next_text, expected, "renaming a sibling preserves comments, Rust expressions, and other blocks");
        assert_eq!(out.focus_open_byte, expected.rfind("Label{").map(|i| i + 5), "focus maps against the rescanned second block");
        assert_eq!(out.file_edits, vec![Replace::new(src.rfind("save :=").unwrap()..src.rfind("save :=").unwrap() + 4, "save_button")], "wire edit covers only the original identifier");
        let p = tree.source().find("#(0)").unwrap();
        let splice = |at: usize, text: &str| {
            let edits = vec![Replace::new(at..at, text)];
            WidgetEdit { syntax: tree.apply(&edits).unwrap(), edits, focus_open_brace: None, focus_name: None }
        };
        for at in [p, p + 4] {
            let out = doc.apply_block_edit(1, splice(at, " ")).unwrap();
            let file_start = src.find("#(my_rust_val)").unwrap();
            let file_at = if at == p { file_start } else { file_start + "#(my_rust_val)".len() };
            assert_eq!(out.file_edits[0].range, file_at..file_at, "both placeholder edges map to their exact file boundary");
        }
        for fake in ["#(0)", "@(0)"] {
            let edit = set_widget_prop(&tree, &Loc(vec![0, 1]), "text", fake, ColorStyle::MacroHashX).unwrap();
            assert!(doc.apply_block_edit(1, edit).is_err(), "cannot inject placeholder form {fake}");
        }
        assert_eq!(doc.find_rust_id_refs("save").iter().map(|s| s.line).collect::<Vec<_>>(), [3, 4], "reference scan finds both Rust macros and skips comments, strings and script bodies");
        let splash = ScriptModDoc::parse("design.splash", "View{}".into(), DocKind::Splash).unwrap();
        assert_eq!(splash.hash(), ScriptModDoc::parse("other", "View{}".into(), DocKind::Splash).unwrap().hash(), "hash depends only on source bytes");
    }

    #[test]
    fn script_mod_doc_theme_tokens_updates_existing_or_inserts_override() {
        let tokens = vec![("color_accent".into(), "#abc".into()), ("size".into(), "12".into())];
        let src = "script_mod!{\n    mod.theme +: {\n        color_accent: #xfff\n        // keep\n        old: #(some_value)\n    }\n}";
        let doc = ScriptModDoc::parse("app.rs", src.into(), DocKind::RustScriptMod).unwrap();
        let out = doc.apply_theme_tokens(0, None, None, &tokens).unwrap();
        assert_eq!(out.next_text, src.replace("#xfff", "#xabc").replace("old: #(some_value)", "old: #(some_value)\n        size: 12"), "existing theme updates in place across an untouched interpolation");
        let mut replay = src.to_string();
        for e in out.file_edits.iter().rev() { replay.replace_range(e.range.clone(), &e.text); }
        assert_eq!(replay, out.next_text, "all token writes replay in original coordinates");
        let src = "script_mod!{\n    use mod.widgets.*\n    App{}\n}";
        let doc = ScriptModDoc::parse("app.rs", src.into(), DocKind::RustScriptMod).unwrap();
        let out = doc.apply_theme_tokens(0, Some("dark"), Some("Evening"), &tokens).unwrap();
        assert_eq!(out.next_text, "script_mod!{\n    use mod.widgets.*\n    mod.theme = mod.themes.dark\n    // theme sheet: Evening\n    mod.theme +: {\n        color_accent: #xabc\n        size: 12\n    }\n    App{}\n}", "new override follows imports with the requested base, sheet, and macro colors");
        for (src, expected) in [
            ("script_mod!{}", "script_mod!{\n    mod.theme +: {\n        color_accent: #xabc\n        size: 12\n    }\n}"),
            ("script_mod!{\n    }", "script_mod!{\n        mod.theme +: {\n            color_accent: #xabc\n            size: 12\n        }\n    }"),
        ] {
            let doc = ScriptModDoc::parse("app.rs", src.into(), DocKind::RustScriptMod).unwrap();
            assert_eq!(doc.apply_theme_tokens(0, None, None, &tokens).unwrap().next_text, expected, "empty macro keeps its closing delimiter and indent");
        }
        let note = [("note".into(), "\"a\nb\"".into())];
        for src in ["script_mod!{\n    use mod.widgets.*\n    App{}\n}", "script_mod!{\n    }"] {
            let doc = ScriptModDoc::parse("app.rs", src.into(), DocKind::RustScriptMod).unwrap();
            let out = doc.apply_theme_tokens(0, None, Some("Notes"), &note).unwrap();
            let next = ScriptModDoc::parse("app.rs", out.next_text, DocKind::RustScriptMod).unwrap();
            let tree = next.block_syntax(0).unwrap();
            let value = tree.statements().iter().find_map(|stmt| match &stmt.kind {
                Kind::Entry { name, value, .. } if name == "mod.theme" => value.field("note"),
                _ => None,
            }).expect("inserted theme contains the note token");
            assert_eq!(value.value.string(), Some("a\nb"), "theme indentation preserves the multiline token value in {src}");
        }
        let src = "mod.themes.dark = { color_accent: #fff }\nmod.theme +: { color_accent: #123 }";
        let doc = ScriptModDoc::parse("theme.splash", src.into(), DocKind::Splash).unwrap();
        let out = doc.apply_theme_tokens(0, Some("dark"), None, &tokens[..1]).unwrap();
        assert_eq!(out.next_text, src.replace("#fff", "#abc"), "named base takes precedence and Splash uses plain hash colors");
    }
}
