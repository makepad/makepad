//! Which top-level definitions of the `script_mod!` blocks a run used.
//!
//! A block's text is split into its top-level statements
//! ([`top_statements`]); after a run ([`top_use`]) each statement that
//! defines an object (`mod.widgets.ButtonFlat = ...`, `mod.themes.light =
//! {...}`, `let Row = View{...}`) is marked used when the heap reaches that
//! object from what Rust holds (the app's widgets' sources, registries,
//! templates looked up by name), through protos, map and vec values and
//! arrays, with the module scopes and preludes as walls (a name merely
//! bound is not a use). A build tool blanks the unused ones in a web build.

use crate::makepad_live_id::*;
use crate::value::*;
use crate::vm::*;
use std::collections::HashSet;

/// A top-level statement of a Splash block: its byte range (from the end
/// of the previous statement, so the comments above it are its own) and
/// what it defines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopStatement {
    pub start: usize,
    pub end: usize,
    /// `["mod", "widgets", "Button"]` for `mod.widgets.Button = ...` or
    /// `+: ...`; `["let", "X"]`, `["fn", "f"]`; `["use", ..]` and anything
    /// else that defines nothing: what its first words are.
    pub path: Vec<String>,
}

impl TopStatement {
    /// The defined name's last segment (`Button`), when it defines one.
    pub fn name(&self) -> Option<&str> {
        match self.path.first().map(String::as_str) {
            Some("mod") if self.path.len() >= 3 => self.path.last().map(String::as_str),
            Some("let") | Some("fn") => self.path.get(1).map(String::as_str),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Lex {
    Code,
    Str,
    Line,
    Block,
}

/// The top-level statements of `code`, in order, covering it end to end.
/// A statement ends at a `;` at depth 0, or before the next line whose
/// first token sits at depth 0 and does not continue an expression (`.`,
/// an operator, a closing bracket, `do`, `else`).
pub fn top_statements(code: &str) -> Vec<TopStatement> {
    let b = code.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut lex = Lex::Code;
    let mut depth: i32 = 0;
    let mut i = 0;
    let mut prev_end = 0;
    // The current statement's first token and its column.
    let mut first: Option<(usize, usize)> = None;
    let mut line_start = 0;
    let mut at_line_start = true;
    // Just past the last code character (strings included, comments not).
    let mut last_code = 0;
    while i < n {
        let c = b[i];
        match lex {
            Lex::Str => {
                if c == b'\\' {
                    i += 2;
                    continue;
                }
                if c == b'"' {
                    lex = Lex::Code;
                    last_code = i + 1;
                }
                if c == b'\n' {
                    line_start = i + 1;
                }
                i += 1;
                continue;
            }
            Lex::Line => {
                if c == b'\n' {
                    lex = Lex::Code;
                    line_start = i + 1;
                    at_line_start = true;
                }
                i += 1;
                continue;
            }
            Lex::Block => {
                if c == b'*' && b.get(i + 1) == Some(&b'/') {
                    lex = Lex::Code;
                    i += 2;
                    continue;
                }
                if c == b'\n' {
                    line_start = i + 1;
                }
                i += 1;
                continue;
            }
            Lex::Code => {}
        }
        if c == b'\n' {
            line_start = i + 1;
            at_line_start = true;
            i += 1;
            continue;
        }
        if c == b' ' || c == b'\t' || c == b'\r' {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            lex = Lex::Line;
            i += 2;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            lex = Lex::Block;
            i += 2;
            continue;
        }
        // A token starts here.
        if depth == 0 {
            match first {
                None => first = Some((i, 0)),
                Some(_) if at_line_start && !continues(&b[i..]) => {
                    // A new statement: the previous one ends with the line
                    // of its last token, so the comments between them
                    // (a doc comment above this one) are this one's.
                    let cut = b[last_code..].iter().position(|&c| c == b'\n').map_or(line_start, |p| last_code + p + 1).min(line_start);
                    out.push(statement(code, prev_end, cut, first.unwrap().0));
                    prev_end = cut;
                    first = Some((i, 0));
                }
                _ => {}
            }
        }
        at_line_start = false;
        last_code = i + 1;
        match c {
            b'"' => lex = Lex::Str,
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => depth -= 1,
            b';' if depth == 0 => {
                if let Some((s, _)) = first.take() {
                    out.push(statement(code, prev_end, i + 1, s));
                    prev_end = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if let Some((s, _)) = first {
        out.push(statement(code, prev_end, n, s));
    } else if prev_end < n {
        // Trailing whitespace and comments belong to the last statement.
        if let Some(last) = out.last_mut() {
            last.end = n;
        }
    }
    out
}

/// Whether a line starting with `rest` continues the expression above.
fn continues(rest: &[u8]) -> bool {
    match rest[0] {
        b'.' | b'+' | b'-' | b'*' | b'/' | b'%' | b'|' | b'&' | b'?' | b':' | b'=' | b'<' | b'>' | b'}' | b']' | b')' | b',' | b';' => true,
        _ => rest.starts_with(b"do ") || rest.starts_with(b"do\n") || rest.starts_with(b"else"),
    }
}

fn statement(code: &str, start: usize, end: usize, first: usize) -> TopStatement {
    TopStatement { start, end, path: head_path(&code[first..end]) }
}

/// The words a statement starts with: `mod.a.b.C =` gives `[mod, a, b, C]`,
/// `let X` and `fn f` their keyword and name.
fn head_path(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        if !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        match c {
            '.' => continue,
            ' ' | '\t' if words.first().is_some_and(|w| w == "let" || w == "fn" || w == "use") && words.len() < 2 => continue,
            _ => break,
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// One block's statements and whether each defined object was reached.
#[derive(Clone, Debug)]
pub struct BlockUse {
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// The block's text as compiled in (`ScriptMod::code`).
    pub code: String,
    /// Each statement, with `Some(reached)` when it defines an object,
    /// `None` when it defines no object (a `use`, a number, a function).
    pub statements: Vec<(TopStatement, Option<bool>)>,
}

/// After a run: every `script_mod!` block with its statements' use. Runs a
/// garbage collection first, so what nothing holds any more is not a use.
pub fn top_use(vm: &mut ScriptVm) -> Vec<BlockUse> {
    vm.gc();
    let heap = &vm.bx.heap;
    let valid = |o: ScriptObject| heap.objects.is_valid(o) && heap.objects[o].tag.is_alloced();
    // The module scopes and the preludes hold every name: walls.
    let mut walls: HashSet<ScriptObject> = HashSet::new();
    walls.insert(heap.modules);
    for (key, m) in heap.objects[heap.modules].map.iter() {
        let Some(module) = m.value.as_object() else { continue };
        if heap.objects[module].proto != *key {
            continue;
        }
        walls.insert(module);
        if key.as_id() == Some(id!(prelude)) {
            walls.extend(heap.objects[module].map.iter().filter_map(|(_, v)| v.value.as_object()));
        }
    }
    // The definitions themselves (every value of a module scope): Rust
    // holds each type's proto from its registration on, which is not a use.
    let mut definitions: HashSet<ScriptObject> = HashSet::new();
    for wall in &walls {
        definitions.extend(heap.objects[*wall].map.iter().filter_map(|(_, v)| v.value.as_object()));
    }
    // A block's scope and `me`, and the child scopes made under them (a
    // function's captured scope among them), hold every name the block
    // bound (`use mod.widgets.*`): binding a name is not a use either.
    let is_block_scope = |o: ScriptObject| {
        let mut proto = heap.objects[o].proto;
        for _ in 0..64 {
            if proto == id!(scope).into() || proto == id!(root_me).into() {
                return true;
            }
            match proto.as_object() {
                Some(p) if valid(p) => proto = heap.objects[p].proto,
                _ => return false,
            }
        }
        false
    };
    let seeds: Vec<ScriptObject> = heap.root_objects.borrow().keys().copied().filter(|o| valid(*o) && !walls.contains(o) && !definitions.contains(o) && !is_block_scope(*o)).collect();
    let mut reached: HashSet<ScriptObject> = HashSet::new();
    let mut stack: Vec<ScriptObject> = Vec::new();
    for s in seeds {
        if reached.insert(s) {
            stack.push(s);
        }
    }
    let mut arrays: HashSet<ScriptArray> = HashSet::new();
    let mut values: Vec<ScriptValue> = Vec::new();
    while let Some(from) = stack.pop() {
        let object = &heap.objects[from];
        values.clear();
        values.push(object.proto);
        values.extend(object.map.iter().map(|(_, e)| e.value));
        values.extend(object.vec.iter().map(|kv| kv.value));
        let mut i = 0;
        while i < values.len() {
            let value = values[i];
            i += 1;
            if let Some(to) = value.as_object() {
                if valid(to) && !walls.contains(&to) && !is_block_scope(to) && reached.insert(to) {
                    stack.push(to);
                }
            } else if let Some(arr) = value.as_array() {
                if heap.arrays.is_valid(arr) && arrays.insert(arr) {
                    for j in 0..heap.array_len(arr) {
                        values.push(heap.array_index_unchecked(arr, j));
                    }
                }
            }
        }
    }
    // The object a statement defines: `mod.a.b.C` from the modules down,
    // `let X` from the block's final scope.
    let lookup = |from: ScriptObject, name: &str| -> Option<ScriptValue> {
        let key: ScriptValue = LiveId::from_str(name).into();
        heap.objects[from].map.iter().find(|(k, _)| **k == key).map(|(_, e)| e.value)
    };
    let mut out = Vec::new();
    for body in vm.bx.code.bodies.borrow().iter() {
        let ScriptSource::Mod(script_mod) = &body.source else { continue };
        let scope = body.end_scope.as_ref().map(|s| s.as_object()).unwrap_or_else(|| body.scope.as_object());
        let statements = top_statements(&script_mod.code)
            .into_iter()
            .map(|s| {
                let value = match s.path.first().map(String::as_str) {
                    Some("mod") if s.path.len() >= 3 => s.path[1..].iter().try_fold(heap.modules.into(), |at: ScriptValue, seg| at.as_object().filter(|o| valid(*o)).and_then(|o| lookup(o, seg))),
                    Some("let") if s.path.len() >= 2 && valid(scope) => lookup(scope, &s.path[1]),
                    _ => None,
                };
                // A prelude or module scope is used by name (`use mod.x.*`).
                let used = value.and_then(|v| v.as_object()).filter(|o| valid(*o)).map(|o| walls.contains(&o) || reached.contains(&o));
                (s, used)
            })
            .collect();
        out.push(BlockUse { file: script_mod.file.clone(), line: script_mod.line, column: script_mod.column, code: script_mod.code.clone(), statements });
    }
    // A block's last statement stays: a block that ends in a `use` and a
    // lone `;` (or nothing) does not evaluate. So does a definition that is
    // a Rust type's registration (`= #(..)`, `= set_type_default() do
    // #(..)`): Rust checks fields against the type by that object.
    for block in &mut out {
        let code = block.code.clone();
        let last = block.statements.len().saturating_sub(1);
        for (i, (s, used)) in block.statements.iter_mut().enumerate() {
            let text = &code[s.start..s.end];
            let rhs = text.split_once('=').map(|(_, r)| r.trim_start()).unwrap_or("");
            let registers = rhs.starts_with("#(") || rhs.starts_with("set_type_default() do #(");
            if *used == Some(false) && (i == last || registers) {
                *used = Some(true);
            }
        }
    }
    // Code refers to definitions by name too (a shader's `CursorShape.Arrow`,
    // a template naming another): a definition whose name appears in a kept
    // statement's text is kept, until nothing changes.
    loop {
        let mut words: HashSet<&str> = HashSet::new();
        for block in &out {
            for (s, used) in &block.statements {
                if *used != Some(false) {
                    words.extend(identifiers(&block.code[s.start..s.end]));
                }
            }
        }
        let mut keep: Vec<(usize, usize)> = Vec::new();
        for (bi, block) in out.iter().enumerate() {
            for (si, (s, used)) in block.statements.iter().enumerate() {
                if *used == Some(false) && s.name().is_some_and(|n| words.contains(n)) {
                    keep.push((bi, si));
                }
            }
        }
        if keep.is_empty() {
            break;
        }
        for (bi, si) in keep {
            out[bi].statements[si].1 = Some(true);
        }
    }
    out
}

/// The identifiers in `text` (strings and comments skipped).
fn identifiers(text: &str) -> impl Iterator<Item = &str> {
    let b = text.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            spans.push((start, i));
        } else {
            i += 1;
        }
    }
    spans.into_iter().map(move |(s, e)| &text[s..e])
}
