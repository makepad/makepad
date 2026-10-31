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
use makepad_relax_api::relax_api;

use makepad_script::makepad_live_id::*;
use makepad_script::value::*;
use makepad_script::heap::ScriptHeap;
use makepad_script::vm::*;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// A top-level statement of a Splash block: its byte range (from the end
/// of the previous statement, so the comments above it are its own) and
/// what it defines.
#[derive(Debug, Eq)]
pub struct TopStatement {
    pub start: usize,
    pub end: usize,
    /// `["mod", "widgets", "Button"]` for `mod.widgets.Button = ...` or
    /// `+: ...`; `["let", "X"]`, `["fn", "f"]`; `["use", ..]` and anything
    /// else that defines nothing: what its first words are.
    pub path: Vec<String>,
}

// The caller builds and reads these collections. Keep their trait operations
// on that same side of the provided-crate boundary.
impl Clone for TopStatement {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self { start: self.start, end: self.end, path: self.path.clone() }
    }
}

impl PartialEq for TopStatement {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        self.start == other.start && self.end == other.end && self.path == other.path
    }
}

impl TopStatement {
    /// The defined name's last segment (`Button`), when it defines one.
    #[inline(always)]
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
#[derive(Debug)]
pub struct BlockUse {
    /// The registering `module_path!()` (`makepad_widgets::button`): with a
    /// statement's path, the stable name of a definition.
    pub module_path: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// The block's text as compiled in (`ScriptMod::code`).
    pub code: String,
    /// Each statement, with `Some(reached)` when it defines an object,
    /// `None` when it defines no object (a `use`, a number, a function).
    pub statements: Vec<(TopStatement, Option<bool>)>,
}

impl Clone for BlockUse {
    #[inline(always)]
    fn clone(&self) -> Self {
        Self {
            module_path: self.module_path.clone(),
            file: self.file.clone(),
            line: self.line,
            column: self.column,
            code: self.code.clone(),
            statements: self.statements.clone(),
        }
    }
}

/// One block of [`top_use_into`]'s answer: where the block is, its text,
/// and per statement of [`top_statements`] of that text 0 (defines no
/// object), 1 (not reached) or 2 (reached).
#[relax_api(plain)]
#[repr(C)]
pub struct BlockReachOut<'a> {
    ctx: *mut u8,
    block: unsafe extern "C-unwind" fn(*mut u8, *const u8, usize, *const u8, usize, usize, usize, *const u8, usize, *const u8, usize),
    _out: std::marker::PhantomData<&'a mut u8>,
}

impl<'a> BlockReachOut<'a> {
    #[inline(always)]
    pub fn new(out: &'a mut Vec<BlockUse>) -> Self {
        #[allow(clippy::too_many_arguments)]
        unsafe extern "C-unwind" fn block(ctx: *mut u8, module: *const u8, module_len: usize, file: *const u8, file_len: usize, line: usize, column: usize, code: *const u8, code_len: usize, reach: *const u8, reach_len: usize) {
            let out = &mut *(ctx as *mut Vec<BlockUse>);
            let text = |p: *const u8, n: usize| std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, n)).to_string();
            let code = text(code, code_len);
            let reach = std::slice::from_raw_parts(reach, reach_len);
            let statements = top_statements(&code)
                .into_iter()
                .zip(reach.iter())
                .map(|(s, r)| (s, match r { 0 => None, 1 => Some(false), _ => Some(true) }))
                .collect();
            out.push(BlockUse { module_path: text(module, module_len), file: text(file, file_len), line, column, code, statements });
        }
        Self { ctx: out as *mut Vec<BlockUse> as *mut u8, block, _out: std::marker::PhantomData }
    }

    fn block(&mut self, module: &str, file: &str, line: usize, column: usize, code: &str, reach: &[u8]) {
        unsafe { (self.block)(self.ctx, module.as_ptr(), module.len(), file.as_ptr(), file.len(), line, column, code.as_ptr(), code.len(), reach.as_ptr(), reach.len()) }
    }
}

/// After a run: every `script_mod!` block with its statements' use. Runs a
/// garbage collection first, so what nothing holds any more is not a use.
pub fn top_use(vm: &mut ScriptVm) -> Vec<BlockUse> {
    let mut out = Vec::new();
    top_use_into(vm, &mut BlockReachOut::new(&mut out));
    keep_named(&mut out);
    out
}

/// The heap half of [`top_use`]: which definitions what Rust holds reaches.
#[relax_api]
pub fn top_use_into(vm: &mut ScriptVm, out: &mut BlockReachOut) {
    vm.gc();
    let heap = &vm.bx.heap;
    let valid = |o: ScriptObject| heap.object_slots().is_valid(o) && heap.object_slots()[o].tag.is_alloced();
    // The module scopes and the preludes hold every name: walls.
    let walls = module_walls(heap);
    // The definitions themselves (every value of a module scope): Rust
    // holds each type's proto from its registration on, which is not a use.
    let mut definitions: HashSet<ScriptObject> = HashSet::new();
    for wall in &walls {
        definitions.extend(heap.object_slots()[*wall].map.iter().filter_map(|(_, v)| v.value.as_object()));
    }
    // A block's scope and `me`, and the child scopes made under them (a
    // function's captured scope among them), hold every name the block
    // bound (`use mod.widgets.*`): binding a name is not a use either.
    let is_block_scope = |o: ScriptObject| {
        let mut proto = heap.object_slots()[o].proto;
        for _ in 0..64 {
            if proto == id!(scope).into() || proto == id!(root_me).into() {
                return true;
            }
            match proto.as_object() {
                Some(p) if valid(p) => proto = heap.object_slots()[p].proto,
                _ => return false,
            }
        }
        false
    };
    let seeds: Vec<ScriptObject> = heap.rooted_objects().into_iter().filter(|o| valid(*o) && !walls.contains(o) && !definitions.contains(o) && !is_block_scope(*o)).collect();
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
        let object = &heap.object_slots()[from];
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
                if heap.array_slots().is_valid(arr) && arrays.insert(arr) {
                    for j in 0..heap.array_len(arr) {
                        values.push(heap.array_index_unchecked(arr, j));
                    }
                }
            }
        }
    }
    for body in vm.bx.code.bodies.borrow().iter() {
        let ScriptSource::Mod(script_mod) = &body.source else { continue };
        let scope = body_scope(body);
        let statements = top_statements(&script_mod.code)
            .into_iter()
            .map(|s| {
                // A prelude or module scope is used by name (`use mod.x.*`).
                let used = defined_object(heap, scope, &s).map(|o| walls.contains(&o) || reached.contains(&o));
                match used { None => 0u8, Some(false) => 1, Some(true) => 2 }
            })
            .collect::<Vec<u8>>();
        out.block(&script_mod.module_path, &script_mod.file, script_mod.line, script_mod.column, &script_mod.code, &statements);
    }
}

/// A block's final scope (where its `let`s are).
fn body_scope(body: &ScriptBody) -> ScriptObject {
    body.end_scope.as_ref().map(|s| s.as_object()).unwrap_or_else(|| body.scope.as_object())
}

/// The object a statement defines: `mod.a.b.C` from the modules down,
/// `let X` from the block's final scope; None when it defines no live object.
fn defined_object(heap: &ScriptHeap, scope: ScriptObject, s: &TopStatement) -> Option<ScriptObject> {
    let valid = |o: ScriptObject| heap.object_slots().is_valid(o) && heap.object_slots()[o].tag.is_alloced();
    let lookup = |from: ScriptObject, name: &str| -> Option<ScriptValue> {
        let key: ScriptValue = LiveId::from_str(name).into();
        heap.object_slots()[from].map.iter().find(|(k, _)| **k == key).map(|(_, e)| e.value)
    };
    let value = match s.path.first().map(String::as_str) {
        Some("mod") if s.path.len() >= 3 => s.path[1..].iter().try_fold(heap.modules.into(), |at: ScriptValue, seg| at.as_object().filter(|o| valid(*o)).and_then(|o| lookup(o, seg))),
        Some("let") if s.path.len() >= 2 && valid(scope) => lookup(scope, &s.path[1]),
        _ => None,
    };
    value.and_then(|v| v.as_object()).filter(|o| valid(*o))
}

/// The module scopes and the preludes' values: used by name, never by a read of their own.
fn module_walls(heap: &ScriptHeap) -> HashSet<ScriptObject> {
    let mut walls: HashSet<ScriptObject> = HashSet::new();
    walls.insert(heap.modules);
    for (key, m) in heap.object_slots()[heap.modules].map.iter() {
        let Some(module) = m.value.as_object() else { continue };
        if heap.object_slots()[module].proto != *key {
            continue;
        }
        walls.insert(module);
        if key.as_id() == Some(id!(prelude)) {
            walls.extend(heap.object_slots()[module].map.iter().filter_map(|(_, v)| v.value.as_object()));
        }
    }
    walls
}

// ---- Touches: what a run read, not what Rust holds (makepad_script::touch).

/// One block's definitions, as every VM that registered it saw them used.
struct TouchBlock {
    module_path: String,
    file: String,
    line: usize,
    column: usize,
    code: String,
    /// per statement of [`top_statements`]: 0 defines no object, 1 not touched, 2 touched
    states: Vec<u8>,
}

#[derive(Default)]
struct TouchRecord {
    blocks: Vec<TouchBlock>,
    /// (file, line, column, text hash) -> index in `blocks`
    index: HashMap<(String, usize, usize, u64), usize>,
    /// a registration's statement (slot) read a definition (slot): kept with it
    edges: HashSet<(u32, u32)>,
}

static TOUCHES: Mutex<Option<TouchRecord>> = Mutex::new(None);

fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// A definition slot: block index and statement index.
const SLOT_BITS: u32 = 16;

fn touch_sink(slot: u32) {
    let Ok(mut record) = TOUCHES.lock() else { return };
    let Some(record) = record.as_mut() else { return };
    if let Some(state) = record.blocks.get_mut((slot >> SLOT_BITS) as usize).and_then(|b| b.states.get_mut((slot & ((1 << SLOT_BITS) - 1)) as usize)) {
        *state = 2;
    }
}

/// Maps the definitions of every evaluated block of `code` not mapped yet: each object a
/// statement defines is announced to the heap by the ip that made it, so its first read
/// reports the statement. `late`: blocks mapped only at the end (whatever ran before was not
/// watched) count as touched.
fn touch_map_bodies(heap: &ScriptHeap, code: &ScriptCode, late: bool) {
    let bodies = code.bodies.borrow();
    // A body that has not run defines nothing yet.
    let new: Vec<usize> = bodies
        .iter()
        .enumerate()
        .filter(|(id, body)| matches!(body.source, ScriptSource::Mod(_)) && body.source_len > 0 && heap.touch_map_body(*id as u16, body.content_hash))
        .map(|(id, _)| id)
        .collect();
    if new.is_empty() {
        if let Ok(mut record) = TOUCHES.lock() {
            if let Some(record) = record.as_mut() {
                touch_edges(heap, &bodies[..], record);
            }
        }
        return;
    }
    let walls = module_walls(heap);
    let Ok(mut record) = TOUCHES.lock() else { return };
    let Some(record) = record.as_mut() else { return };
    for id in new {
        let body = &bodies[id];
        let ScriptSource::Mod(m) = &body.source else { continue };
        let statements = top_statements(&m.code);
        let key = (m.file.clone(), m.line, m.column, fnv64(m.code.as_bytes()));
        let at = match record.index.get(&key) {
            Some(&at) => at,
            None => {
                record.blocks.push(TouchBlock { module_path: m.module_path.clone(), file: m.file.clone(), line: m.line, column: m.column, code: m.code.clone(), states: vec![1; statements.len()] });
                record.index.insert(key, record.blocks.len() - 1);
                record.blocks.len() - 1
            }
        };
        if statements.len() >= 1 << SLOT_BITS || at >= 1 << (32 - SLOT_BITS) {
            // Too big to slot: everything in it stays.
            record.blocks[at].states.iter_mut().for_each(|s| *s = 2);
            continue;
        }
        let scope = body_scope(body);
        for (si, s) in statements.iter().enumerate() {
            let state = match defined_object(heap, scope, s) {
                None => 0,
                Some(o) if walls.contains(&o) => 2,
                Some(o) => {
                    let made_at = heap.made_at(o);
                    if late || made_at.is_unknown() {
                        2
                    } else {
                        if heap.touch_define(o, made_at, ((at as u32) << SLOT_BITS) | si as u32) { 2 } else { 1 }
                    }
                }
            };
            // Kept when any VM keeps it: touched, or no object there.
            let merged = &mut record.blocks[at].states[si];
            *merged = match (*merged, state) {
                (2, _) | (_, 2) => 2,
                (0, _) | (_, 0) => 0,
                _ => 1,
            };
        }
    }
    touch_edges(heap, &bodies[..], record);
}

/// The statement of `body` (its top-level statements `statements`) the opcode `index` belongs to.
fn statement_at(body: &ScriptBody, statements: &[TopStatement], index: u32) -> Option<usize> {
    let source_map = &body.parser.source_map;
    let token = source_map.get(index as usize).copied().flatten().or_else(|| source_map[..(index as usize).min(source_map.len())].iter().rev().find_map(|t| *t))?;
    let char_pos = body.tokenizer.token_char_pos(token)?;
    let byte = body.effective_code.char_indices().nth(char_pos).map_or(body.effective_code.len(), |(b, _)| b);
    statements.iter().position(|s| s.start <= byte && byte < s.end)
}

/// The registrations' reads as edges between definition slots.
fn touch_edges(heap: &ScriptHeap, bodies: &[ScriptBody], record: &mut TouchRecord) {
    let edges = heap.touch_take_edges();
    if edges.is_empty() {
        return;
    }
    let mut statements: HashMap<u16, Option<(usize, Vec<TopStatement>)>> = HashMap::new();
    for (reader, target) in edges {
        let targets = heap.touch_slots(target);
        if targets.is_empty() {
            continue;
        }
        let at = statements.entry(reader.body).or_insert_with(|| {
            let body = bodies.get(reader.body as usize)?;
            let ScriptSource::Mod(m) = &body.source else { return None };
            let block = *record.index.get(&(m.file.clone(), m.line, m.column, fnv64(m.code.as_bytes())))?;
            Some((block, top_statements(&m.code)))
        });
        let Some((block, stmts)) = at else { continue };
        let Some(si) = statement_at(&bodies[reader.body as usize], stmts, reader.index) else { continue };
        let from = ((*block as u32) << SLOT_BITS) | si as u32;
        for to in targets {
            if to != from {
                record.edges.insert((from, to));
            }
        }
    }
}

fn touch_map(heap: &ScriptHeap, code: &ScriptCode) {
    touch_map_bodies(heap, code, false);
}

/// Records touches from now on: in `vm` and in every VM made after (a document's own). Before
/// the first script module.
#[relax_api]
pub fn touch_record(vm: &mut ScriptVm) {
    if let Ok(mut record) = TOUCHES.lock() {
        record.get_or_insert_with(Default::default);
    }
    let _ = makepad_script::touch::TOUCH_MAP.set(touch_map);
    let _ = makepad_script::touch::TOUCH_SINK.set(touch_sink);
    makepad_script::touch::TOUCH_RECORD.store(true, std::sync::atomic::Ordering::Relaxed);
    vm.bx.heap.touch_start();
    touch_map_bodies(&vm.bx.heap, &vm.bx.code, false);
}

/// After a run: every block any VM evaluated, with its statements' use by touches
/// ([`touch_record`] first; empty without it): read outside a registration, or read by a kept
/// statement's registration, or kept for what it is (a type's registration, a block's last).
pub fn touch_use(vm: &mut ScriptVm) -> Vec<BlockUse> {
    let mut out = Vec::new();
    touch_use_into(vm, &mut BlockReachOut::new(&mut out));
    out
}

/// The record half of [`touch_use`].
#[relax_api]
pub fn touch_use_into(vm: &mut ScriptVm, out: &mut BlockReachOut) {
    touch_map_bodies(&vm.bx.heap, &vm.bx.code, true);
    let Ok(record) = TOUCHES.lock() else { return };
    let Some(record) = record.as_ref() else { return };
    let mut states: Vec<Vec<u8>> = record.blocks.iter().map(|b| b.states.clone()).collect();
    // A type's registration stays: Rust checks fields against the type by that object.
    let mut block_statements: Vec<Vec<TopStatement>> = Vec::with_capacity(record.blocks.len());
    for (bi, b) in record.blocks.iter().enumerate() {
        let statements = top_statements(&b.code);
        for (si, s) in statements.iter().enumerate() {
            if states[bi].get(si) == Some(&1) && registers(&b.code[s.start..s.end]) {
                states[bi][si] = 2;
            }
        }
        block_statements.push(statements);
    }
    let mut next: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(from, to) in &record.edges {
        next.entry(from).or_default().push(to);
    }
    let slot_state = |states: &Vec<Vec<u8>>, slot: u32| states.get((slot >> SLOT_BITS) as usize).and_then(|b| b.get((slot & ((1 << SLOT_BITS) - 1)) as usize)).copied();
    // What a kept statement read while it registered is kept with it.
    let close = |states: &mut Vec<Vec<u8>>, mut stack: Vec<u32>| {
        while let Some(from) = stack.pop() {
            for &to in next.get(&from).map(Vec::as_slice).unwrap_or(&[]) {
                if slot_state(states, to) == Some(1) {
                    states[(to >> SLOT_BITS) as usize][(to & ((1 << SLOT_BITS) - 1)) as usize] = 2;
                    stack.push(to);
                }
            }
        }
    };
    let roots: Vec<u32> = next.keys().copied().filter(|&s| matches!(slot_state(&states, s), Some(0) | Some(2))).collect();
    close(&mut states, roots);
    // A block that would end in a `use` (or in nothing) does not evaluate: then its last
    // statement stays. A block ending in any other kept statement ends well (the blanked rest
    // is spaces).
    let mut lasts = Vec::new();
    for (bi, statements) in block_statements.iter().enumerate() {
        let st = &mut states[bi];
        let Some(last) = st.len().checked_sub(1) else { continue };
        let ends_in_use = match st.iter().rposition(|&k| k != 1) {
            None => true,
            Some(i) => statements.get(i).and_then(|s| s.path.first()).map(String::as_str) == Some("use"),
        };
        if ends_in_use && st[last] == 1 {
            st[last] = 2;
            lasts.push(((bi as u32) << SLOT_BITS) | last as u32);
        }
    }
    close(&mut states, lasts);
    for (b, st) in record.blocks.iter().zip(&states) {
        out.block(&b.module_path, &b.file, b.line, b.column, &b.code, st);
    }
}

/// Whether a statement is a Rust type's registration (`= #(..)`, `= set_type_default() do
/// #(..)`): Rust checks fields against the type by that object.
fn registers(text: &str) -> bool {
    let rhs = text.split_once('=').map(|(_, r)| r.trim_start()).unwrap_or("");
    rhs.starts_with("#(") || rhs.starts_with("set_type_default() do #(")
}

/// The text half of [`top_use`]: statements kept for what they are or for
/// being named by a kept one.
fn keep_named(out: &mut [BlockUse]) {
    // A block's last statement stays: a block that ends in a `use` and a
    // lone `;` (or nothing) does not evaluate. So does a definition that is
    // a Rust type's registration (`= #(..)`, `= set_type_default() do
    // #(..)`): Rust checks fields against the type by that object.
    for block in out.iter_mut() {
        let code = block.code.clone();
        let last = block.statements.len().saturating_sub(1);
        for (i, (s, used)) in block.statements.iter_mut().enumerate() {
            if *used == Some(false) && (i == last || registers(&code[s.start..s.end])) {
                *used = Some(true);
            }
        }
    }
    // Code refers to definitions by name too (a shader's `CursorShape.Arrow`,
    // a template naming another): a definition whose name appears in a kept
    // statement's text is kept, until nothing changes.
    loop {
        let mut words: HashSet<&str> = HashSet::new();
        for block in out.iter() {
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
