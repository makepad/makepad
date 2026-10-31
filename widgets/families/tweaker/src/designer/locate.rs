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
use crate::makepad_draw::makepad_platform::makepad_script::tokenizer::{ScriptToken, ScriptTokenTag, ScriptTokenizer};
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
        let made_at = vm.heap().made_at(source);
        if made_at.is_unknown() {
            return Err("the widget's source was built by Rust, not by a literal".to_string());
        }
        if made_at.body as usize >= vm.bx.body_count() {
            return Err("the widget's source body is gone".to_string());
        }
        let body = vm.bx.body_module_read(made_at.body as usize);
        let Some(script_mod) = body.module() else {
            return Err("the widget's source is not a script_mod! block".to_string());
        };
        let Some(token_index) = body.token_index(made_at.index as usize) else {
            return Err("the literal's instruction maps to no token".to_string());
        };
        let Some(file) = Cx::resolve_script_mod_path(script_mod) else {
            return Err(format!("the source file {} is not on disk here", script_mod.view().file));
        };
        let tokens: Vec<ScriptToken> = body.tokens().iter().map(|t| *t.token()).collect();
        Ok((file, script_mod.view().line, token_index as usize, tokens))
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
        tokenizer.tokenize(&block.code, vm.heap_mut());
    });
    let file_tokens: Vec<ScriptToken> = tokenizer.tokens_ref().iter().map(|t| *t.token()).collect();
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
        let Some(script_mod) = body.module() else {
            return Err("the widget's source is not a script_mod! block".to_string());
        };
        Cx::resolve_script_mod_path(script_mod)
            .ok_or_else(|| format!("the source file {} is not on disk here", script_mod.view().file))
    })
}
