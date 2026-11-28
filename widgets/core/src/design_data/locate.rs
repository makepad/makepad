//! From a live widget to the source literal that declares it.
//!
//! The runtime already knows where every object literal came from: the
//! object's `made_at` is the instruction that built it, the body's source map
//! turns that into a token, and the body's tokenizer knows the token's row and
//! column in the code it compiled. That code is the macro's reconstruction of
//! the block or a hot-reloaded override, never the file's bytes, so the last
//! step goes through a fresh scan of the file: the scanned block is tokenized
//! again and its token stream is compared with the runtime's. When they agree
//! token for token, the same token index names the same token in both, and
//! the scanned block knows the file byte under it. When they do not, the file
//! has moved on since the app was built or previewed, and the widget cannot
//! be located; nothing is guessed.

use std::ops::Range;
use crate::makepad_draw::makepad_platform::live_reload::{scan_script_mods, ScriptModBlock};
use crate::makepad_draw::makepad_platform::makepad_script::tokenizer::{ScriptToken, ScriptTokenTag, ScriptTokenPos, ScriptTokenizer};
use crate::makepad_platform::makepad_script::syntax::widget::{find_object_by_open_brace, is_fixed_slot, parent_field_of};
use crate::makepad_platform::script_mod_doc::{DocKind, ScriptModDoc};
use crate::makepad_draw::*;
use crate::{view::View, widget_tree::live_id_token};
use crate::widget::*;

/// Where a widget's literal lives: the file, the text it was located in, the
/// block inside that text, and the node.
#[derive(Clone, Debug)]
pub struct NodeSpan {
    /// The resolved path of the file.
    pub file: String,
    /// The text the span indexes: the file as read, or the text handed in.
    pub text: String,
    /// The scanned blocks of `text`; `block` indexes them.
    pub blocks: Vec<ScriptModBlock>,
    pub block: usize,
    pub node: Node,
}

/// Locate the literal that declares `widget`. With `text`, the file's
/// current working text (a design document mid-edit) is scanned instead of
/// the disk; the runtime is then expected to run that text's override.
pub fn locate_widget(cx: &mut Cx, widget: &WidgetRef, text: Option<&str>, mode: LocateMode) -> Result<NodeSpan, String> {
    let (doc, block, open, _) = locate_open(cx, widget, text, mode)?;
    let node = node_at_brace(&doc.text, open)
        .ok_or_else(|| "the literal's braces do not match".to_string())?;
    check_placeholders(&doc, block, node.range(), mode)?;
    Ok(NodeSpan { file: doc.path, text: doc.text, blocks: doc.blocks, block, node })
}

#[derive(Clone, Debug)]
pub struct LocatedWidget {
    pub file: String,
    pub text: String,
    pub from_preview: bool,
    pub block: usize,
    pub open_byte: usize,
    pub range: Range<usize>,
    pub file_hash: u64,
    pub node_hash: u64,
    pub is_container: bool,
    pub flow: &'static str,
    pub is_slot: bool,
}

pub fn is_container_type_name(ty: &str) -> bool {
    matches!(ty, "View" | "Grid" | "Masonry" | "KeyboardView")
}

pub fn is_container(cx: &Cx, widget: &WidgetRef) -> bool {
    if widget.borrow::<View>().is_some() { return true; }
    let Some(id) = widget.widget_type_id() else { return false };
    let registry = cx.components.get::<WidgetRegistry>();
    registry.map.get(&id).is_some_and(|(info, _)| {
        is_container_type_name(live_id_token(info.name).as_str())
    })
}

pub fn locate_widget_ast(cx: &mut Cx, widget: &WidgetRef, explicit_text: Option<&str>, mode: LocateMode)
    -> Result<LocatedWidget, String>
{
    let (doc, block, open_byte, from_preview) = locate_open(cx, widget, explicit_text, mode)?;
    let syntax = doc.block_syntax(block)?;
    let open = doc.block_offset_from_file(block, open_byte)?;
    let loc = find_object_by_open_brace(&syntax, open).ok_or("no widget object at the running brace")?;
    let span = if let Some((parent, index)) = parent_field_of(&syntax, &loc) {
        syntax.node(&parent).and_then(|n| n.object()).and_then(|(_, fields)| fields.get(index)).map(|f| &f.span)
    } else {
        syntax.node(&loc).map(|n| &n.span)
    }.ok_or("no source span for widget object")?;
    let range = doc.file_offset_from_block(block, span.start)?..doc.file_offset_from_block(block, span.end)?;
    check_placeholders(&doc, block, range.clone(), mode)?;
    let flow = match widget.borrow::<View>().map(|v| v.layout.flow) {
        Some(Flow::Right { .. }) => "row",
        Some(Flow::Down) => "col",
        Some(Flow::Overlay) => "overlay",
        None => "none",
    };
    Ok(LocatedWidget {
        file_hash: doc.hash(), node_hash: source_hash(&doc.text[range.clone()]), file: doc.path, text: doc.text,
        block, open_byte, range, from_preview, is_container: is_container(cx, widget), flow, is_slot: is_fixed_slot(&syntax, &loc),
    })
}

fn check_placeholders(doc: &ScriptModDoc, block: usize, range: Range<usize>, mode: LocateMode) -> Result<(), String> {
    if mode == LocateMode::Strict {
        if let Some(opaque) = doc.blocks[block].placeholders.iter().find(|r| range.contains(&r.start)) {
            return Err(format!("{}: cannot verify Rust interpolation at bytes {}..{}: the running module retains only its slot",
                doc.path, opaque.start, opaque.end));
        }
    }
    Ok(())
}

fn locate_open(cx: &mut Cx, widget: &WidgetRef, text: Option<&str>, mode: LocateMode)
    -> Result<(ScriptModDoc, usize, usize, bool), String>
{
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Err("the widget has no script source".to_string());
    }
    // 1. The runtime side: which file, which token, and the token stream the
    //    runtime compiled.
    let (file, key, token_index, runtime_tokens, running_source) = cx.with_vm(|vm| {
        let made_at = vm.heap().made_at(source);
        if made_at.is_unknown() {
            return Err("the widget's source was built by Rust, not by a literal".to_string());
        }
        if made_at.body as usize >= vm.bx.body_count() {
            return Err("the widget's source body is gone".to_string());
        }
        let body = vm.bx.body_module_read(made_at.body as usize);
        let Some(script_mod) = body.script_mod() else {
            return Err("the widget's source is not a script_mod! block".to_string());
        };
        let Some(token_index) = body.token_index(made_at.index as usize) else {
            return Err("the literal's instruction maps to no token".to_string());
        };
        let Some(file) = Cx::resolve_script_mod_path(script_mod) else {
            return Err(format!("the source file {} is not on disk here", script_mod.view().file));
        };
        let tokens = body.tokens().to_vec();
        let running_source = (mode == LocateMode::Strict).then(|| body.view().code.to_owned());
        Ok((file, makepad_script::ScriptModKey::from_script_mod(script_mod), token_index as usize, tokens, running_source))
    })?;

    // 2. The file side.
    let (text, from_preview) = match text {
        Some(text) => (text.to_owned(), false),
        None => match cx.active_preview_text(&file) {
            Some(text) => (text, true),
            None => (std::fs::read_to_string(&file).map_err(|e| format!("{}: {}", file, e))?, false),
        },
    };
    let blocks = scan_script_mods(&file, &text)?;
    let index = if blocks.len() > 1 {
        cx.preview_block_index(&file, &key, blocks.len())
            .or_else(|| pick_block(&blocks, key.line()))
    } else {
        pick_block(&blocks, key.line())
    };
    let Some(block_index) = index else {
        return Err(format!("{}: no script_mod! block for the site at line {}", file, key.line()));
    };
    let block = &blocks[block_index];

    // 3. Align the two token streams.
    let tokenizer = cx.with_vm(|vm| {
        let mut tokenizer = match mode {
            LocateMode::Strict => ScriptTokenizer::with_units(vm.unit_table()),
            LocateMode::Lenient => ScriptTokenizer::default(),
        };
        tokenizer.tokenize(&block.code, vm.heap_mut());
        tokenizer
    });
    let file_tokens = tokenizer.tokens_ref();
    if let Some(source) = running_source {
        check_strict(&file, block, &source, &runtime_tokens, file_tokens)?;
    } else if let Some(at) = first_mismatch(&runtime_tokens, file_tokens) {
        return Err(format!(
            "{}: the file no longer matches what runs (first difference at token {}); build or preview it first",
            file, at
        ));
    }
    if token_index >= file_tokens.len() {
        return Err("the literal's token lies past the end of the file's block".to_string());
    }

    // 4. The token's byte in the file, then the literal around it.
    let Some((row, col)) = tokenizer.token_index_to_row_col(token_index as u32) else {
        return Err("the literal's token has no position".to_string());
    };
    let code_offset = row_col_to_offset(&block.code, row as usize, col as usize)
        .ok_or_else(|| "the literal's position lies outside its block".to_string())?;
    let file_offset = block.code_to_file_offset(code_offset);
    let open = brace_from(&text, file_offset, block.body_end)
        .ok_or_else(|| "no `{` follows the literal's token".to_string())?;
    Ok((ScriptModDoc { path: file, text, blocks, kind: DocKind::RustScriptMod }, block_index, open, from_preview))
}

/// The block whose first token is on the runtime's recorded line, else the
/// nearest one.
fn pick_block(blocks: &[ScriptModBlock], line: usize) -> Option<usize> {
    if let Some(i) = blocks.iter().position(|b| b.first_token_line == line) {
        return Some(i);
    }
    blocks
        .iter()
        .enumerate()
        .min_by_key(|(_, b)| (b.first_token_line as isize - line as isize).abs())
        .map(|(i, _)| i)
}

/// The index of the first token the two streams disagree on, ignoring the
/// end markers; `None` when they agree.
fn first_mismatch(a: &[ScriptTokenPos], b: &[ScriptTokenPos]) -> Option<usize> {
    let a: Vec<&ScriptToken> = a.iter().map(ScriptTokenPos::token).filter(|t| !is_end(t)).collect();
    let b: Vec<&ScriptToken> = b.iter().map(ScriptTokenPos::token).filter(|t| !is_end(t)).collect();
    if a.len() != b.len() {
        return Some(a.len().min(b.len()));
    }
    a.iter().zip(&b).position(|(x, y)| !same_token(x, y))
}

fn is_end(t: &ScriptToken) -> bool {
    matches!(t.kind(), ScriptTokenTag::End | ScriptTokenTag::StreamEnd)
}

/// Token equality by kind and literal value. Strings compare by kind only:
/// their values are heap handles, and a string that changed still leaves the
/// structure in place, which is what the locator needs.
fn same_token(a: &ScriptToken, b: &ScriptToken) -> bool {
    use ScriptTokenTag::*;
    if a.kind() != b.kind() { return false; }
    match a.kind() {
        Identifier => a.identifier() == b.identifier(),
        Operator => a.operator() == b.operator(),
        Separator => a.separator() == b.separator(),
        OpenCurly | CloseCurly | OpenRound | CloseRound | OpenSquare | CloseSquare | StringUnfinished | String => true,
        F32 => a.as_f32().unwrap().to_bits() == b.as_f32().unwrap().to_bits(),
        F16 => a.as_f16().unwrap().to_bits() == b.as_f16().unwrap().to_bits(),
        F64 => a.as_f64().unwrap().to_bits() == b.as_f64().unwrap().to_bits(),
        U32 => a.as_u32() == b.as_u32(),
        I32 => a.as_i32() == b.as_i32(),
        U40 => a.as_u40() == b.as_u40(),
        Color => a.as_color() == b.as_color(),
        RustValue => a.as_rust_value() == b.as_rust_value(),
        _ => false,
    }
}

/// The byte offset of a (row, column) in `code`; the column counts chars.
fn row_col_to_offset(code: &str, row: usize, col: usize) -> Option<usize> {
    let mut line_start = 0;
    for (i, line) in code.split_inclusive('\n').enumerate() {
        if i == row {
            let line_body = line.strip_suffix('\n').unwrap_or(line);
            let byte = line_body
                .char_indices()
                .nth(col)
                .map(|(b, _)| b)
                .unwrap_or(line_body.len());
            return Some(line_start + byte);
        }
        line_start += line.len();
    }
    None
}

/// The `{` at or after `from`, before `limit`: the token the runtime maps a
/// literal to is its brace or the type path before it.
fn brace_from(text: &str, from: usize, limit: usize) -> Option<usize> {
    let b = text.as_bytes();
    let mut i = from;
    while i < limit.min(b.len()) {
        match b[i] {
            b'{' => return Some(i),
            b'\n' | b'}' | b';' => return None,
            _ => i += 1,
        }
    }
    None
}

/// The resolved path of the file `widget` was declared in, without locating
/// the literal.
pub fn widget_source_file(cx: &mut Cx, widget: &WidgetRef) -> Result<String, String> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Err("the widget has no script source".to_string());
    }
    cx.with_vm(|vm| {
        let made_at = vm.heap().made_at(source);
        if made_at.is_unknown() {
            return Err("the widget's source was built by Rust, not by a literal".to_string());
        }
        let body = vm.bx.body_module_read(made_at.body as usize);
        if !body.exists() {
            return Err("the widget's source body is gone".to_string());
        };
        let Some(script_mod) = body.script_mod() else {
            return Err("the widget's source is not a script_mod! block".to_string());
        };
        Cx::resolve_script_mod_path(script_mod)
            .ok_or_else(|| format!("the source file {} is not on disk here", script_mod.view().file))
    })
}

/// What a `{` opens, judged from the text before it.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// `Type{...}` or `name := Type{...}`: a child literal.
    Child { name: Option<String> },
    /// `key: {...}` or `key +: {...}`: a property object, not a child.
    Property { key: String, merge: bool },
    /// `path = Type{...}` or `let x = Type{...}`: an assignment.
    Assign,
    /// Anything else (a block after `do`, a fn body, an argument).
    Other,
}

/// One literal: `start..end` is the whole declaration, `open` its `{` and
/// `close` its `}`, `ty` the type path before the brace.
#[derive(Clone, Debug)]
pub struct Node {
    pub start: usize,
    pub open: usize,
    pub close: usize,
    pub ty: String,
    pub kind: NodeKind,
}

impl Node {
    pub fn end(&self) -> usize {
        self.close + 1
    }
    pub fn range(&self) -> Range<usize> {
        self.start..self.end()
    }
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Child { name } => name.as_deref(),
            _ => None,
        }
    }
}

/// Skip a string, comment or `#(...)` placeholder that starts at `i`, or
/// return `None` when nothing opaque starts there.
pub fn skip_opaque(b: &[u8], i: usize) -> Option<usize> {
    match b[i] {
        b'"' => {
            let mut j = i + 1;
            while j < b.len() {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => return Some(j + 1),
                    _ => j += 1,
                }
            }
            Some(b.len())
        }
        b'r' if b.get(i + 1) == Some(&b'"') || (b.get(i + 1) == Some(&b'#')) => {
            // Raw string r"..." or r#"..."#.
            let mut hashes = 0;
            let mut j = i + 1;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                return None;
            }
            j += 1;
            while j < b.len() {
                if b[j] == b'"' {
                    let mut k = j + 1;
                    let mut n = 0;
                    while n < hashes && b.get(k) == Some(&b'#') {
                        n += 1;
                        k += 1;
                    }
                    if n == hashes {
                        return Some(k);
                    }
                }
                j += 1;
            }
            Some(b.len())
        }
        b'/' if b.get(i + 1) == Some(&b'/') => {
            let mut j = i;
            while j < b.len() && b[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        b'/' if b.get(i + 1) == Some(&b'*') => {
            let mut j = i + 2;
            while j + 1 < b.len() {
                if b[j] == b'*' && b[j + 1] == b'/' {
                    return Some(j + 2);
                }
                j += 1;
            }
            Some(b.len())
        }
        b'#' if b.get(i + 1) == Some(&b'(') => {
            let mut depth = 0usize;
            let mut j = i + 1;
            while j < b.len() {
                if let Some(k) = if b[j] == b'"' || (b[j] == b'/' && j + 1 < b.len()) {
                    skip_opaque(b, j)
                } else {
                    None
                } {
                    j = k;
                    continue;
                }
                match b[j] {
                    b'(' => depth += 1,
                    b')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(j + 1);
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            Some(b.len())
        }
        _ => None,
    }
}

/// The index of the `}` matching the `{` at `open`.
pub fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let b = text.as_bytes();
    if b.get(open) != Some(&b'{') {
        return None;
    }
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        if let Some(j) = skip_opaque(b, i) {
            i = j;
            continue;
        }
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub fn is_ident_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// Walk back over whitespace from `i` (exclusive), returning the index after
/// the last non-space byte.
fn rskip_ws(b: &[u8], mut i: usize) -> usize {
    while i > 0 && (b[i - 1] == b' ' || b[i - 1] == b'\t') {
        i -= 1;
    }
    i
}

/// The type path ending at `end` (exclusive): idents joined by dots.
fn rpath(b: &[u8], end: usize) -> usize {
    let mut i = end;
    while i > 0 && (is_ident_byte(b[i - 1]) || b[i - 1] == b'.') {
        i -= 1;
    }
    i
}

/// Describe the literal whose `{` is at `open`.
pub fn node_at_brace(text: &str, open: usize) -> Option<Node> {
    let b = text.as_bytes();
    let close = matching_brace(text, open)?;
    let ty_end = rskip_ws(b, open);
    let ty_start = rpath(b, ty_end);
    let ty = text[ty_start..ty_end].to_string();
    // Before the type path: `name :=`, `key:`, `key +:`, `=`, or the start of
    // a statement.
    let before = rskip_ws(b, ty_start);
    let kind = if ty.is_empty() {
        // A bare `{`: `key: {` / `key +: {` / `= {` / `do {`.
        if before >= 2 && &b[before - 2..before] == b"+:" {
            let key_end = rskip_ws(b, before - 2);
            let key_start = rpath(b, key_end);
            NodeKind::Property { key: text[key_start..key_end].to_string(), merge: true }
        } else if before >= 1 && b[before - 1] == b':' {
            let key_end = rskip_ws(b, before - 1);
            let key_start = rpath(b, key_end);
            NodeKind::Property { key: text[key_start..key_end].to_string(), merge: false }
        } else if before >= 1 && b[before - 1] == b'=' {
            NodeKind::Assign
        } else {
            NodeKind::Other
        }
    } else if before >= 2 && &b[before - 2..before] == b":=" {
        let name_end = rskip_ws(b, before - 2);
        let name_start = rpath(b, name_end);
        NodeKind::Child { name: Some(text[name_start..name_end].to_string()) }
    } else if before >= 2 && &b[before - 2..before] == b"+:" {
        let key_end = rskip_ws(b, before - 2);
        let key_start = rpath(b, key_end);
        NodeKind::Property { key: text[key_start..key_end].to_string(), merge: true }
    } else if before >= 1 && b[before - 1] == b':' {
        let key_end = rskip_ws(b, before - 1);
        let key_start = rpath(b, key_end);
        NodeKind::Property { key: text[key_start..key_end].to_string(), merge: false }
    } else if before >= 1 && b[before - 1] == b'=' {
        NodeKind::Assign
    } else if before >= 2 && &b[before - 2..before] == b"do" {
        NodeKind::Other
    } else {
        NodeKind::Child { name: None }
    };
    let start = match &kind {
        NodeKind::Child { name: Some(_) } => {
            let name_end = rskip_ws(b, before - 2);
            rpath(b, name_end)
        }
        NodeKind::Property { .. } => {
            let colon = if before >= 2 && &b[before - 2..before] == b"+:" {
                before - 2
            } else {
                before - 1
            };
            let key_end = rskip_ws(b, colon);
            rpath(b, key_end)
        }
        _ => ty_start,
    };
    Some(Node { start, open, close, ty, kind })
}


pub fn source_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in text.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LocateMode {
    Lenient,
    Strict,
}

/// Compare the lexemes of the effective (possibly hot-reloaded) source. Token
/// positions count characters. File edits and the public answer count bytes.
fn check_strict(
    file: &str,
    block: &ScriptModBlock,
    running: &str,
    runtime_tokens: &[ScriptTokenPos],
    disk_tokens: &[ScriptTokenPos],
) -> Result<(), String> {
    fn byte_offsets(source: &str) -> Vec<usize> {
        source.char_indices().map(|(byte, _)| byte).chain(std::iter::once(source.len())).collect()
    }
    fn bytes(token: &ScriptTokenPos, offsets: &[usize]) -> Option<Range<usize>> {
        Some(*offsets.get(token.start())?..*offsets.get(token.end())?)
    }
    let runtime_offsets = byte_offsets(running);
    let disk_offsets = byte_offsets(&block.code);
    let mut runtime = runtime_tokens.iter().filter(|t| !is_end(t.token()));
    let mut disk = disk_tokens.iter().filter(|t| !is_end(t.token()));
    for index in 0..=runtime_tokens.len().max(disk_tokens.len()) {
        let (left, right) = (runtime.next(), disk.next());
        if left.is_none() && right.is_none() { return Ok(()); }
        let old = left.map(|t| bytes(t, &runtime_offsets)
            .and_then(|r| running.get(r)).ok_or("running token has no source span")).transpose()?;
        let range = right.map(|t| bytes(t, &disk_offsets)
            .ok_or("file token has no source span")).transpose()?;
        let new = range.clone().map(|r| &block.code[r]);
        if old != new || left.map(|t| t.token().kind()) != right.map(|t| t.token().kind()) {
            let range = range.map(|r| {
                block.code_to_file_offset(r.start).min(block.body_end)..block.code_to_file_offset(r.end).min(block.body_end)
            }).unwrap_or(block.body_end..block.body_end);
            return Err(format!("{file}: token {index} differs at bytes {}..{}: running {:?}, file {:?}",
                range.start, range.end, old.unwrap_or("<end>"), new.unwrap_or("<end>")));
        }
    }
    Ok(())
}

pub(super) fn remote(cx: &mut Cx, widget: &WidgetRef) -> Result<String, String> {
    let w = locate_widget_ast(cx, widget, None, LocateMode::Strict)?;
    Ok(format!(r#"{{"file":{},"start":{},"end":{},"hash":"{:016x}","open":{},"block":{},"file_hash":"{:016x}","preview":{},"is_container":{},"flow":"{}","is_slot":{}}}"#,
        super::json_str(&w.file), w.range.start, w.range.end, w.node_hash, w.open_byte, w.block, w.file_hash, w.from_preview, w.is_container, w.flow, w.is_slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    mod first_block {
        use crate::makepad_draw::*;
        script_mod! { 1 }
    }

    mod fixture {
        use crate::makepad_draw::*;
        script_mod! {
            let seed = #(123.0)
            mod.widgets.Label{text: "Save"}
        }
    }

    fn changed_string() -> (crate::test_cx::PooledCx, WidgetRef, NodeSpan, String, usize) {
        let mut cx = crate::test_cx::checkout_test_cx(crate::script_mod);
        let widget = cx.with_vm(|vm| {
            first_block::script_mod(vm);
            let source = fixture::script_mod(vm);
            WidgetRef::script_from_value(vm, source)
        });
        let span = locate_widget(&mut cx, &widget, None, LocateMode::Strict).unwrap();
        let at = span.node.start + span.text[span.node.range()].find("\"Save\"").unwrap();
        let mut disk = span.text.clone();
        disk.replace_range(at..at + 6, "\"Send\"");
        (cx, widget, span, disk, at)
    }

    #[test]
    fn locate_ast_finds_fixture_when_first_block_never_ran() {
        let mut cx = Cx::new(Box::new(|_, _| {}));
        cx.init_cx_os();
        cx.with_vm(crate::script_mod);
        let widget = cx.with_vm(|vm| {
            let source = fixture::script_mod(vm);
            WidgetRef::script_from_value(vm, source)
        });
        let eval_widget = cx.with_vm(|vm| {
            let source = crate::script_eval!(vm, { mod.widgets.Label{text: "Eval"} });
            WidgetRef::script_from_value(vm, source)
        });
        let located = locate_widget_ast(&mut cx, &widget, None, LocateMode::Strict)
            .expect("strict locate should find the fixture after an unexecuted block");
        assert_eq!(located.block, 1, "the fixture is the second block in the file");
        let err = locate_widget_ast(&mut cx, &eval_widget, None, LocateMode::Strict)
            .expect_err("an evaluated widget has no static block");
        assert!(err.contains("not a script_mod! block"), "unexpected locate error: {err}");
    }

    #[test]
    fn locate_ast_matches_active_preview_while_disk_is_unchanged() {
        use super::super::preview::{apply_preview, PreviewPayload};
        let (mut cx, _, span, _, _) = changed_string();
        let disk = span.text;
        let sibling = r#"added := mod.widgets.Label{text: "Added"}"#;
        let mut text = disk.clone();
        let fixed = "input: mod.widgets.View{}";
        text.replace_range(span.node.range(), &format!("mod.widgets.View{{ first := {}\n{sibling}\nwell := mod.widgets.FieldWell{{{fixed}}}\n}}", &disk[span.node.range()]));
        cx.design_preview_callback = Some(|cx, file, text| Some(cx.install_live_edit_text(file, text).map(|_| ())));
        apply_preview(&mut cx, &span.file, PreviewPayload::FullText(text.clone())).unwrap();
        let root = cx.with_vm(|vm| {
            let value = vm.with_reload(fixture::script_mod);
            WidgetRef::script_from_value(vm, value)
        });
        crate::set_ui_root(&mut cx, &root);
        let label = root.child(id!(added));
        let w = locate_widget_ast(&mut cx, &label, None, LocateMode::Strict).unwrap();
        let start = text.find(sibling).unwrap();
        assert!(w.from_preview, "locate reads the installed preview");
        assert_eq!(w.range, start..start + sibling.len(), "the entry maps to exact file bytes after the interpolation");
        assert_eq!(w.open_byte, start + sibling.find('{').unwrap(), "open maps to the constructor brace");
        assert_eq!(w.node_hash, source_hash(sibling), "node hash includes the named entry");
        assert!(!w.is_slot, "a named child is movable");

        // Shift the first block onto the second block's original line.
        let drift = span.blocks[1].first_token_line - span.blocks[0].first_token_line;
        let shifted = format!("{}{text}", "\n".repeat(drift));
        let explicit = locate_widget_ast(&mut cx, &label, Some(&shifted), LocateMode::Strict)
            .expect("strict locate must match the compiled site in shifted explicit text");
        assert_eq!(explicit.block, 1, "the compiled key still selects the second block");
        assert_eq!(explicit.range, start + drift..start + drift + sibling.len(), "the entry range follows the shifted explicit text");

        let well = root.child(id!(well));
        let input = well.borrow::<crate::field_well::FieldWell>().unwrap().input.clone();
        let slot = locate_widget_ast(&mut cx, &input, None, LocateMode::Strict).unwrap();
        assert!(slot.is_slot && slot.is_container, "a View in a fixed slot accepts insertion inside");
        assert_eq!(&slot.text[slot.range], fixed, "the fixed slot range covers its field name and constructor");
        let verified = super::super::callback(&mut cx, "design_verify", &[("file".into(), w.file.clone()), ("path".into(), "added".into())]).unwrap();
        assert!(verified.contains(&format!(r#""open":{}"#, w.open_byte)), "verify round-trips the rebuilt sibling");
        assert_eq!(std::fs::read_to_string(&w.file).unwrap(), disk, "locating the new sibling leaves disk untouched");
        apply_preview(&mut cx, &span.file, PreviewPayload::Revert).unwrap();
        cx.design_preview_callback = None;
    }

    #[test]
    fn strict_locate_reports_the_first_changed_strings_bytes_and_both_texts() {
        let (mut cx, widget, span, disk, at) = changed_string();
        let result = remote(&mut cx, &widget).unwrap();
        let range = span.node.range();
        let hash = source_hash(&span.text[range.clone()]);
        assert!(result.contains(&format!(r#""start":{},"end":{},"hash":"{hash:016x}""#, range.start, range.end)), "{result}");
        let err = locate_widget(&mut cx, &widget, Some(&disk), LocateMode::Strict).unwrap_err();
        assert!(err.contains(&format!("bytes {at}..{}", at + 6)), "{err}");
        assert!(err.contains("Save") && err.contains("Send"), "{err}");
    }

    #[test]
    fn lenient_locate_keeps_the_literal_across_a_changed_string() {
        let (mut cx, widget, span, disk, _) = changed_string();
        let located = locate_widget(&mut cx, &widget, Some(&disk), LocateMode::Lenient).unwrap();
        assert_eq!(located.node.range(), span.node.range());
    }
}
