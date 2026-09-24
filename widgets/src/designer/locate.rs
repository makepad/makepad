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

use super::text::{self, Node};
use crate::makepad_draw::makepad_platform::live_reload::{scan_script_mods, ScriptModBlock};
use crate::makepad_draw::makepad_platform::makepad_script::tokenizer::{ScriptToken, ScriptTokenizer};
use crate::makepad_draw::makepad_platform::makepad_script::ScriptSource;
use crate::makepad_draw::*;
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
pub fn locate_widget(cx: &mut Cx, widget: &WidgetRef, text: Option<&str>) -> Result<NodeSpan, String> {
    let source = widget.script_source();
    if source == ScriptObject::ZERO {
        return Err("the widget has no script source".to_string());
    }
    // 1. The runtime side: which file, which token, and the token stream the
    //    runtime compiled.
    let (file, mod_line, token_index, runtime_tokens) = cx.with_vm(|vm| {
        let made_at = vm.bx.heap.object_data(source).made_at;
        if made_at.is_unknown() {
            return Err("the widget's source was built by Rust, not by a literal".to_string());
        }
        let bodies = vm.bx.code.bodies.borrow();
        let Some(body) = bodies.get(made_at.body as usize) else {
            return Err("the widget's source body is gone".to_string());
        };
        let ScriptSource::Mod(script_mod) = &body.source else {
            return Err("the widget's source is not a script_mod! block".to_string());
        };
        let Some(token_index) = body
            .parser
            .source_map
            .get(made_at.index as usize)
            .copied()
            .flatten()
        else {
            return Err("the literal's instruction maps to no token".to_string());
        };
        let Some(file) = Cx::resolve_script_mod_path(script_mod) else {
            return Err(format!("the source file {} is not on disk here", script_mod.file));
        };
        let tokens: Vec<ScriptToken> = body.tokenizer.tokens.iter().map(|t| t.token).collect();
        Ok((file, script_mod.line, token_index as usize, tokens))
    })?;

    // 2. The file side.
    let text = match text {
        Some(text) => text.to_string(),
        None => std::fs::read_to_string(&file).map_err(|e| format!("{}: {}", file, e))?,
    };
    let blocks = scan_script_mods(&file, &text)?;
    let Some(block_index) = pick_block(&blocks, mod_line) else {
        return Err(format!("{}: no script_mod! block at line {}", file, mod_line));
    };
    let block = &blocks[block_index];

    // 3. Align the two token streams.
    let mut tokenizer = ScriptTokenizer::default();
    cx.with_vm(|vm| {
        tokenizer.tokenize(&block.code, &mut vm.bx.heap);
    });
    let file_tokens: Vec<ScriptToken> = tokenizer.tokens.iter().map(|t| t.token).collect();
    if let Some(at) = first_mismatch(&runtime_tokens, &file_tokens) {
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
    let node = text::node_at_brace(&text, open)
        .ok_or_else(|| "the literal's braces do not match".to_string())?;
    Ok(NodeSpan { file, text, blocks, block: block_index, node })
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
fn first_mismatch(a: &[ScriptToken], b: &[ScriptToken]) -> Option<usize> {
    let a: Vec<&ScriptToken> = a.iter().filter(|t| !is_end(t)).collect();
    let b: Vec<&ScriptToken> = b.iter().filter(|t| !is_end(t)).collect();
    if a.len() != b.len() {
        return Some(a.len().min(b.len()));
    }
    a.iter().zip(&b).position(|(x, y)| !same_token(x, y))
}

fn is_end(t: &ScriptToken) -> bool {
    matches!(t, ScriptToken::End | ScriptToken::StreamEnd)
}

/// Token equality by kind and literal value. Strings compare by kind only:
/// their values are heap handles, and a string that changed still leaves the
/// structure in place, which is what the locator needs.
fn same_token(a: &ScriptToken, b: &ScriptToken) -> bool {
    use ScriptToken::*;
    match (a, b) {
        (Identifier(x), Identifier(y)) | (Operator(x), Operator(y)) | (Separator(x), Separator(y)) => x == y,
        (OpenCurly, OpenCurly)
        | (CloseCurly, CloseCurly)
        | (OpenRound, OpenRound)
        | (CloseRound, CloseRound)
        | (OpenSquare, OpenSquare)
        | (CloseSquare, CloseSquare)
        | (StringUnfinished, StringUnfinished)
        | (String(_), String(_)) => true,
        (F32(x), F32(y)) | (F16(x), F16(y)) => x.to_bits() == y.to_bits(),
        (F64(x), F64(y)) => x.to_bits() == y.to_bits(),
        (U32(x), U32(y)) | (Color(x), Color(y)) | (RustValue(x), RustValue(y)) => x == y,
        (I32(x), I32(y)) => x == y,
        (U40(x), U40(y)) => x == y,
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
