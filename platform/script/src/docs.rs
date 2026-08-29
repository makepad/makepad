//! `/** ... */` doc-annotation metadata. ONE form; position determines
//! meaning.
//!
//! The splash tokenizer records each `/** ... */` block keyed by the index
//! of the NEXT token (`ScriptTokenizer::docs`); the parser never sees them.
//! Every script object carries the ip of the BEGIN_PROTO / BEGIN_BARE
//! opcode that constructed it (`ScriptObjectData::made_at`), so a doc
//! resolves to an attachment point purely at query time, against (tokens,
//! opcodes, source_map) — nothing is stored in the opcode stream and there
//! is no on-disk artifact: metadata is rebuilt from source at every
//! compile, so it can never drift. Hot reload compiles into new bodies;
//! old ips stay resolvable against the retained old body.
//!
//! Position rules (resolve_docs / resolve_value_names):
//! - before `key: value`, `name := Type{...}` or `key +: {...}`: a FIELD
//!   doc on the enclosing object literal.
//! - immediately before a value literal (`/**glow tint*/ #8f0`, numbers,
//!   strings, true/false, a leading `-`): a VALUE NAME for that literal —
//!   the tweaker's Constants rows.
//! - anywhere else: an OBJECT doc on the next object literal that begins
//!   within the following few source tokens (`DOC_ATTACH_WINDOW`).
//! - consecutive blocks with the same attachment merge in order; block
//!   content may span lines (a leading `*` gutter per line is stripped).
//! - a doc that resolves to nothing (e.g. above an array element or at the
//!   very end of a body) is dropped.
//!
//! `//` and `/* */` stay plain comments; `///` is NOT an annotation form.
//!
//! The cascade query (`ScriptVm::construction_chain`) walks an object's
//! proto chain and returns one level per prototype: where it was
//! constructed, its docs, and the keys it sets itself. Rust-built objects
//! (made_at == ScriptIp::UNKNOWN) appear as levels with no location — the
//! tweaker renders those as "native".

use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::tokenizer::ScriptToken;
use crate::tokenizer::ScriptTokenizer;
use crate::value::*;
use crate::vm::ScriptCode;
use crate::vm::ScriptLoc;
use crate::vm::ScriptVm;

/// How many source tokens after a non-field doc line an object literal may
/// begin and still claim the doc. Generous enough for
/// `mod.widgets.X = set_type_default() do mod.widgets.XBase{`.
const DOC_ATTACH_WINDOW: u32 = 256;

/// Longest proto chain construction_chain will walk.
const MAX_CHAIN: usize = 24;

/// One `/** ... */` doc (possibly several merged blocks) resolved to its
/// attachment point inside one body.
#[derive(Clone, Debug)]
pub struct ScriptDocEntry {
    /// Opcode index (within the body) of the BEGIN_PROTO / BEGIN_BARE
    /// opcode of the object literal this doc belongs to. An object built
    /// there carries the same value in `made_at.index`.
    pub begin_index: u32,
    /// None: the doc describes the object itself. Some(key): it describes
    /// that field / named template of the object.
    pub field: Option<LiveId>,
    pub text: String,
}

/// One level of an object's construction chain (the object itself first,
/// then its prototypes outward).
#[derive(Debug)]
pub struct ScriptChainLevel {
    /// The object at this level.
    pub object: ScriptObject,
    /// Construction site; ScriptIp::UNKNOWN for Rust-built objects.
    pub made_at: ScriptIp,
    /// Source location of the construction site (None for native levels).
    pub loc: Option<ScriptLoc>,
    /// Doc annotation attached to the object literal itself.
    pub doc: Option<String>,
    /// Doc annotations attached to fields of this literal.
    pub field_docs: Vec<(LiveId, String)>,
    /// Keys this level sets itself: map keys plus named vec templates.
    pub own_keys: Vec<LiveId>,
}

fn is_field_op(id: LiveId) -> bool {
    id == id!(:) || id == id!(:=) || id == id!(+:)
}

/// Tokens a doc annotation can NAME (the `/**glow tint*/ #8f0` form).
fn is_value_token(tok: &ScriptToken) -> bool {
    match tok {
        ScriptToken::F32(_)
        | ScriptToken::F64(_)
        | ScriptToken::F16(_)
        | ScriptToken::U32(_)
        | ScriptToken::I32(_)
        | ScriptToken::U40(_)
        | ScriptToken::Color(_)
        | ScriptToken::String(_) => true,
        ScriptToken::Identifier(id) => *id == id!(true) || *id == id!(false),
        _ => false,
    }
}

/// Strip per-line whitespace and a Rust-style leading `*` gutter from a
/// block's content.
fn clean_block_text(text: &str) -> String {
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        let line = line.strip_prefix('*').map(str::trim_start).unwrap_or(line);
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn as_begin(v: ScriptValue) -> Option<Opcode> {
    match v.as_opcode() {
        Some((op, _))
            if op == Opcode::BEGIN_PROTO
                || op == Opcode::BEGIN_BARE
                || op == Opcode::BEGIN_ARRAY =>
        {
            Some(op)
        }
        _ => None,
    }
}

fn is_end(v: ScriptValue) -> bool {
    matches!(
        v.as_opcode(),
        Some((Opcode::END_PROTO, _)) | Some((Opcode::END_BARE, _)) | Some((Opcode::END_ARRAY, _))
    )
}

/// Resolve a body's captured `///` lines to doc entries. Pure function of
/// the compile artifacts; called at query time (doc lists are small).
pub fn resolve_docs(
    tokenizer: &ScriptTokenizer,
    opcodes: &[ScriptValue],
    source_map: &[Option<u32>],
) -> Vec<ScriptDocEntry> {
    // first opcode whose source token is at or after `t`. source_map is
    // near-monotonic (operators emit late); a linear scan is exact.
    let first_op_at = |t: u32| -> Option<usize> {
        (0..opcodes.len()).find(|i| matches!(source_map.get(*i), Some(Some(tok)) if *tok >= t))
    };

    let mut out: Vec<ScriptDocEntry> = Vec::new();
    for d in &tokenizer.docs {
        let t = d.next_token;
        let text = clean_block_text(&d.text);
        let toks = &tokenizer.tokens;

        // VALUE NAME position (`/**name*/ 0.35`): resolve_value_names.
        if toks
            .get(t as usize)
            .is_some_and(|tp| is_value_token(&tp.token))
        {
            continue;
        }
        if toks
            .get(t as usize)
            .is_some_and(|tp| matches!(tp.token, ScriptToken::Operator(id) if id == id!(-)))
            && toks
                .get(t as usize + 1)
                .is_some_and(|tp| is_value_token(&tp.token))
        {
            continue;
        }

        let resolved = 'resolve: {
            // FIELD doc: `ident` followed by `:`, `:=` or `+:`.
            if let (Some(a), Some(b)) = (toks.get(t as usize), toks.get(t as usize + 1)) {
                if let ScriptToken::Identifier(name) = a.token {
                    let op = match b.token {
                        ScriptToken::Operator(id) | ScriptToken::Separator(id) => id,
                        _ => LiveId(0),
                    };
                    if is_field_op(op) {
                        // enclosing literal: walk back from the first opcode
                        // of this statement, depth-matching END/BEGIN pairs.
                        let Some(i) = first_op_at(t) else {
                            break 'resolve None;
                        };
                        let mut depth = 0usize;
                        for j in (0..i).rev() {
                            if is_end(opcodes[j]) {
                                depth += 1;
                            } else if let Some(op) = as_begin(opcodes[j]) {
                                if depth == 0 {
                                    if op == Opcode::BEGIN_ARRAY {
                                        break; // docs in arrays: unsupported
                                    }
                                    break 'resolve Some((j as u32, Some(name)));
                                }
                                depth -= 1;
                            }
                        }
                        break 'resolve None;
                    }
                }
            }
            // OBJECT doc: next object literal within the window.
            let Some(start) = first_op_at(t) else {
                break 'resolve None;
            };
            for i in start..opcodes.len() {
                let Some(Some(tok)) = source_map.get(i) else {
                    continue;
                };
                if *tok < t {
                    continue;
                }
                if tok.saturating_sub(t) > DOC_ATTACH_WINDOW {
                    break;
                }
                match as_begin(opcodes[i]) {
                    Some(Opcode::BEGIN_ARRAY) => break,
                    Some(_) => break 'resolve Some((i as u32, None)),
                    None => (),
                }
            }
            None
        };

        if let Some((begin_index, field)) = resolved {
            // merge with an existing entry for the same attachment
            if let Some(prev) = out
                .iter_mut()
                .find(|e| e.begin_index == begin_index && e.field == field)
            {
                prev.text.push('\n');
                prev.text.push_str(&text);
            } else {
                out.push(ScriptDocEntry {
                    begin_index,
                    field,
                    text,
                });
            }
        }
    }
    out
}

/// The lenient hint micro-grammar inside the one `/** */` form:
/// `name [min..max] [step s]` — e.g. `/**pulse speed 0..2 step 0.05*/`.
/// Range and step are HINTS for the editor's bounded scrubber, never
/// clamps: the UI may scrub past a bound (expanding it) or type anything.
/// Unrecognized words stay part of the name. Applies to value-position
/// annotations and to field/definition docs alike.
#[derive(Clone, Debug)]
pub struct ScriptDocHint {
    pub name: String,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// Parse the hint micro-grammar out of a doc text, leniently.
pub fn parse_doc_hint(text: &str) -> ScriptDocHint {
    let mut name_parts: Vec<&str> = Vec::new();
    let (mut min, mut max, mut step) = (None, None, None);
    let mut words = text.split_whitespace().peekable();
    while let Some(w) = words.next() {
        if min.is_none() {
            if let Some((a, b)) = w.split_once("..") {
                if let (Ok(a), Ok(b)) = (a.parse::<f64>(), b.parse::<f64>()) {
                    min = Some(a);
                    max = Some(b);
                    continue;
                }
            }
        }
        if step.is_none() && w == "step" {
            if let Some(next) = words.peek() {
                if let Ok(v) = next.parse::<f64>() {
                    step = Some(v);
                    words.next();
                    continue;
                }
            }
        }
        name_parts.push(w);
    }
    ScriptDocHint {
        name: name_parts.join(" "),
        min,
        max,
        step,
    }
}

/// A `/**name*/` annotation in value position: a human-authored friendly
/// name for the literal that immediately follows it (an inline shader
/// constant, usually). Feeds the tweaker's Constants rows; where no human
/// wrote one, the AI annotation pass may add it.
#[derive(Clone, Debug)]
pub struct ScriptValueName {
    /// Token index of the annotated literal (past a leading `-`).
    pub token: u32,
    pub name: String,
}

/// The value-position annotations of a tokenizer, in token order: docs
/// whose next token is a value literal (or `-` then one).
pub fn resolve_value_names(tokenizer: &ScriptTokenizer) -> Vec<ScriptValueName> {
    let toks = &tokenizer.tokens;
    tokenizer
        .docs
        .iter()
        .filter_map(|d| {
            let t = d.next_token as usize;
            if toks.get(t).is_some_and(|tp| is_value_token(&tp.token)) {
                return Some(ScriptValueName {
                    token: d.next_token,
                    name: clean_block_text(&d.text),
                });
            }
            if toks
                .get(t)
                .is_some_and(|tp| matches!(tp.token, ScriptToken::Operator(id) if id == id!(-)))
                && toks.get(t + 1).is_some_and(|tp| is_value_token(&tp.token))
            {
                return Some(ScriptValueName {
                    token: d.next_token + 1,
                    name: clean_block_text(&d.text),
                });
            }
            None
        })
        .collect()
}

/// The value-position annotation naming the literal at `token`, if any:
/// a doc whose next token is that literal, or a `-` directly before it
/// (`/**x*/ -0.5` names the `0.5`). The shader compiler asks this per
/// immediate when it lifts annotated literals into its constant table.
pub fn value_name_at(tokenizer: &ScriptTokenizer, token: u32) -> Option<String> {
    let toks = &tokenizer.tokens;
    if !toks.get(token as usize).is_some_and(|tp| is_value_token(&tp.token)) {
        return None;
    }
    tokenizer.docs.iter().find_map(|d| {
        if d.next_token == token {
            return Some(clean_block_text(&d.text));
        }
        if d.next_token + 1 == token
            && toks.get(d.next_token as usize).is_some_and(|tp| {
                matches!(tp.token, ScriptToken::Operator(id) if id == id!(-))
            })
        {
            return Some(clean_block_text(&d.text));
        }
        None
    })
}

impl ScriptCode {
    /// The `/**name*/` value-position annotations of one body.
    pub fn resolve_body_value_names(&self, body_index: u16) -> Vec<ScriptValueName> {
        let bodies = self.bodies.borrow();
        let Some(body) = bodies.get(body_index as usize) else {
            return Vec::new();
        };
        resolve_value_names(&body.tokenizer)
    }

    /// Doc entries of one body, resolved fresh from its compile artifacts.
    pub fn resolve_body_docs(&self, body_index: u16) -> Vec<ScriptDocEntry> {
        let bodies = self.bodies.borrow();
        let Some(body) = bodies.get(body_index as usize) else {
            return Vec::new();
        };
        resolve_docs(
            &body.tokenizer,
            &body.parser.opcodes,
            &body.parser.source_map,
        )
    }
}

impl<'a> ScriptVm<'a> {
    /// The construction chain of a value: the object itself, then each
    /// prototype outward. Each level resolves its `made_at` ip to a source
    /// location and its `///` docs. This is the tweaker cascade view's
    /// data source.
    pub fn construction_chain(&self, value: ScriptValue) -> Vec<ScriptChainLevel> {
        let mut out = Vec::new();
        let mut cur = value;
        while let Some(obj) = cur.as_object() {
            if out.len() >= MAX_CHAIN {
                break;
            }
            let data = self.bx.heap.object_data(obj);
            let made_at = data.made_at;
            let mut own_keys: Vec<LiveId> =
                data.map.keys().filter_map(|k| k.as_id()).collect();
            for kv in &data.vec {
                if let Some(id) = kv.key.as_id() {
                    own_keys.push(id);
                }
            }
            let (loc, doc, field_docs) = if made_at.is_unknown() {
                (None, None, Vec::new())
            } else {
                let docs = self.bx.code.resolve_body_docs(made_at.body);
                let mut doc = None;
                let mut field_docs = Vec::new();
                for e in docs {
                    if e.begin_index != made_at.index {
                        continue;
                    }
                    match e.field {
                        None => doc = Some(e.text),
                        Some(f) => field_docs.push((f, e.text)),
                    }
                }
                (self.bx.code.ip_to_loc(made_at), doc, field_docs)
            };
            out.push(ScriptChainLevel {
                object: obj,
                made_at,
                loc,
                doc,
                field_docs,
                own_keys,
            });
            cur = data.proto;
        }
        out
    }
}
