//! Edit mode's literal-site map: every place a literal written in an edited
//! file lands, so an editor's change of that literal reaches all of them for
//! the next frame without a reload or a recompile.
//!
//! A literal is named by its [`LiteralSite`]: the file (by the name its
//! compiled copies give it) and the zero-based line and column of its first
//! character, in the text the running program was loaded from. Its value
//! lands in:
//! - the VM's code (the immediate that pushes it: per-frame code computes
//!   with it again each frame) and what load-time code stored it in as is
//!   (a `let`, a record's field), patched by the host that owns the VM;
//! - shader slots: compiled under edit mode, a shader reads the literal from
//!   a uniform slot instead of a folded constant (`Cx::patch_literal`);
//! - kernel parameters: a compute kernel reads it from a hidden parameter.
//!
//! Each of these is a [`LiteralSink`]; [`patch_sinks`] reaches every live
//! sink. A value computed from the literal while the program loaded (mixed,
//! passed to a call, a loop bound) is not anywhere it can be patched: the
//! sink says so in [`LiteralReach::reload`] and the editor reloads then.
//! Off (no edited files), nothing is lifted, nothing is recorded, and
//! compiled code is byte-identical to a build without the map.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

/// Where a literal is written: a file and the zero-based line and column
/// (in characters) of its first character (a number's digits, past any
/// sign; a colour's `#`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LiteralSite {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// A literal's new value, as an editor writes it: a number (the digits as
/// written, without a sign before them) or a colour's four channels (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LiteralValue {
    Number(f64),
    Color([f32; 4]),
}

/// What a patch reached: how many places now hold the new value, and why a
/// reload is still needed when the literal also fed something that cannot
/// be patched (a value computed from it while the program loaded).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiteralReach {
    pub places: usize,
    pub reload: Vec<String>,
}

impl LiteralReach {
    pub fn add(&mut self, other: LiteralReach) {
        self.places += other.places;
        for why in other.reload {
            if !self.reload.contains(&why) {
                self.reload.push(why);
            }
        }
    }

    /// Whether the editor must reload to show the value everywhere.
    pub fn needs_reload(&self) -> bool {
        !self.reload.is_empty()
    }
}

/// Rows `rows` of a compiled text (zero-based, as its tokenizer or parser
/// counts them) are a copy of `file` from `line`, `col` on: the copy begins
/// at column `code_col` of its first row (text before it there is the
/// host's), and the rows after it are the file's lines as written.
#[derive(Clone, Debug, PartialEq)]
pub struct LiteralOrigin {
    pub rows: std::ops::Range<u32>,
    pub code_col: u32,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

impl LiteralOrigin {
    /// The origins of `copy` (text written in `file` from zero-based
    /// `line`, `col` on) inside a host's generated `code`, row by row: the
    /// copy's first row ends a row of `code`, and every row after it that
    /// reads as written is the file's next line. Rows the host rewrote (a
    /// substitution) are left out, so their literals stay folded rather
    /// than be named by a place they are not at. Empty when the copy's
    /// first row is not in `code`.
    pub fn of_copy(code: &str, copy: &str, file: &str, line: u32, col: u32) -> Vec<LiteralOrigin> {
        let rows: Vec<&str> = code.split('\n').collect();
        let lines: Vec<&str> = copy.split('\n').collect();
        let Some(first) = lines.first().filter(|l| !l.trim().is_empty()) else {
            return Vec::new();
        };
        let Some(r0) = rows.iter().position(|row| row.ends_with(first)) else {
            return Vec::new();
        };
        let mut out: Vec<LiteralOrigin> = Vec::new();
        for (k, text) in lines.iter().enumerate() {
            let r = r0 + k;
            let Some(row) = rows.get(r) else { break };
            if k > 0 && row != text {
                continue;
            }
            let r = r as u32;
            match out.last_mut() {
                Some(o) if k > 0 && o.rows.end == r && o.line + (r - o.rows.start) == line + k as u32 => o.rows.end = r + 1,
                _ => out.push(LiteralOrigin {
                    rows: r..r + 1,
                    code_col: if k == 0 { (row.chars().count() - first.chars().count()) as u32 } else { 0 },
                    file: file.to_string(),
                    line: line + k as u32,
                    col: if k == 0 { col } else { 0 },
                }),
            }
        }
        out
    }
}

/// Edit mode's state: the files being edited, and the compiled texts (by
/// the module name they are compiled under) whose rows are copies of a
/// file. A literal on such a row of an edited file is live.
#[derive(Clone, Debug, Default)]
pub struct LiveLiterals {
    pub edited: Vec<String>,
    pub sources: HashMap<String, Vec<LiteralOrigin>>,
}

impl LiveLiterals {
    /// The site of the literal at `row`, `col` of compiled text `module`,
    /// if that row is a copy of an edited file.
    pub fn site(&self, module: &str, row: u32, col: u32) -> Option<LiteralSite> {
        let origin = self.sources.get(module)?.iter().find(|o| o.rows.contains(&row) && self.is_live(&o.file))?;
        site_in(origin, row, col)
    }

    /// Whether literals copied from `file` are live: an edited file, or a
    /// host's generated text ([`GENERATED`]).
    pub fn is_live(&self, file: &str) -> bool {
        file.starts_with(GENERATED) || self.edited.iter().any(|f| f == file)
    }

    /// The site of the literal at `row`, `col` of a text whose origins are
    /// `origins` (a host's own text, not registered by module), if live.
    pub fn site_in(&self, origins: &[LiteralOrigin], row: u32, col: u32) -> Option<LiteralSite> {
        let origin = origins.iter().find(|o| o.rows.contains(&row) && self.is_live(&o.file))?;
        site_in(origin, row, col)
    }
}

fn site_in(origin: &LiteralOrigin, row: u32, col: u32) -> Option<LiteralSite> {
    let col = if row == origin.rows.start { (col.checked_sub(origin.code_col)?) + origin.col } else { col };
    Some(LiteralSite { file: origin.file.clone(), line: origin.line + (row - origin.rows.start), col })
}

/// The file-name prefix of a host's generated text (values a host baked
/// into code it wrote, not a file anyone edits): live whenever edit mode
/// is on, patched by that host as its values change.
pub const GENERATED: &str = "generated:";

static ON: AtomicBool = AtomicBool::new(false);
static LIVE: RwLock<Option<Arc<LiveLiterals>>> = RwLock::new(None);

/// Whether edit mode is on (one atomic load: what hot paths check).
pub fn edit_mode() -> bool {
    ON.load(Ordering::Relaxed)
}

/// The edit-mode state compilers read (None: off).
pub fn live() -> Option<Arc<LiveLiterals>> {
    if !edit_mode() {
        return None;
    }
    LIVE.read().ok()?.clone()
}

fn update(f: impl FnOnce(&mut LiveLiterals) -> bool) -> bool {
    let Ok(mut guard) = LIVE.write() else { return false };
    let mut next = guard.as_deref().cloned().unwrap_or_default();
    if !f(&mut next) {
        return false;
    }
    ON.store(!next.edited.is_empty(), Ordering::Relaxed);
    *guard = Some(Arc::new(next));
    true
}

/// Edit mode for the files `edited` (none: off). Whether it changed.
pub fn set_edited(edited: Vec<String>) -> bool {
    update(|live| {
        if live.edited == edited {
            return false;
        }
        live.edited = edited;
        true
    })
}

/// Name the rows of compiled text `module` that are copies of a file
/// (empty: forget it). Whether it changed.
pub fn set_source(module: &str, origins: Vec<LiteralOrigin>) -> bool {
    update(|live| {
        if live.sources.get(module).map_or(origins.is_empty(), |old| *old == origins) {
            return false;
        }
        if origins.is_empty() {
            live.sources.remove(module);
        } else {
            live.sources.insert(module.to_string(), origins);
        }
        true
    })
}

/// A place literals land that a patch can reach (a compiled kernel, a
/// running program's VM): it writes `value` everywhere `site` landed in it
/// and says what it reached.
pub trait LiteralSink: Send + Sync {
    fn patch(&self, site: &LiteralSite, value: LiteralValue) -> LiteralReach;
}

static SINKS: Mutex<Vec<Weak<dyn LiteralSink>>> = Mutex::new(Vec::new());

/// Register a sink for as long as it lives (only in edit mode: off, the
/// map holds nothing).
pub fn add_sink(sink: Weak<dyn LiteralSink>) {
    if let Ok(mut sinks) = SINKS.lock() {
        sinks.retain(|s| s.strong_count() > 0);
        sinks.push(sink);
    }
}

/// Patch every live sink (dropped ones are forgotten).
pub fn patch_sinks(site: &LiteralSite, value: LiteralValue) -> LiteralReach {
    let sinks: Vec<Arc<dyn LiteralSink>> = match SINKS.lock() {
        Ok(mut sinks) => {
            sinks.retain(|s| s.strong_count() > 0);
            sinks.iter().filter_map(|s| s.upgrade()).collect()
        }
        Err(_) => Vec::new(),
    };
    let mut reach = LiteralReach::default();
    for sink in sinks {
        reach.add(sink.patch(site, value));
    }
    reach
}

/// The zero-based row and column (characters) of byte `at` of `text`.
pub fn row_col(text: &str, at: usize) -> (u32, u32) {
    let at = at.min(text.len());
    let line_start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    (text[..line_start].matches('\n').count() as u32, text[line_start..at].chars().count() as u32)
}

/// The literal immediates a program's load ran (body, opcode index): what
/// the VM records while [`crate::vm::ScriptVmBase::literal_trace`] is set.
#[derive(Clone, Debug, Default)]
pub struct LoadTrace {
    pub ran: std::collections::HashSet<(u16, u32)>,
}

/// Where a literal's value went in a program that has loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralUse {
    /// Its code did not run at load: per-frame code pushes it each time it
    /// runs, so patching the code is all it takes.
    Code,
    /// Load bound it to a `let` as is.
    Let,
    /// Load stored it as is in a record's field (`{alpha: 0.55}`).
    Field,
    /// Load stored it as is in a list (`[#ff0000, 0.5]`).
    ListItem,
    /// Load computed with it (an operator, a call, a condition): the value
    /// it made is not the literal, and only a reload makes it again.
    Computed,
}

impl LiteralUse {
    /// Why a reload is needed, when it is.
    pub fn reload_reason(self) -> Option<&'static str> {
        match self {
            LiteralUse::Computed => Some("computed with while the program loaded"),
            LiteralUse::ListItem => Some("copied into a list while the program loaded"),
            _ => None,
        }
    }
}

/// What became of the literal immediate at `index` of code body `body`, given
/// what the load ran: its next instruction says (a `-` before it is applied
/// after the immediate and still counts as the literal as written).
pub fn load_use(code: &crate::vm::ScriptCode, trace: &LoadTrace, body: u16, index: u32) -> LiteralUse {
    use crate::opcode::Opcode;
    if !trace.ran.contains(&(body, index)) {
        return LiteralUse::Code;
    }
    let bodies = code.bodies.borrow();
    let Some(b) = bodies.get(body as usize) else { return LiteralUse::Computed };
    let ops = &b.parser.opcodes;
    let mut k = index as usize + 1;
    if matches!(ops.get(k).and_then(|o| o.as_opcode()), Some((Opcode::NEG, _))) {
        k += 1;
    }
    match ops.get(k).and_then(|o| o.as_opcode()) {
        Some((Opcode::LET_DYN | Opcode::LET_TYPED | Opcode::LET_SLOT, _)) => LiteralUse::Let,
        Some((op, _)) if op == Opcode::ASSIGN_ME || op == Opcode::ASSIGN_ME_VEC => LiteralUse::Field,
        Some((Opcode::POP_TO_ME, _)) => LiteralUse::ListItem,
        _ => LiteralUse::Computed,
    }
}

/// One literal an edit changed: where it is in the text before (zero-based
/// line and column of its first character, the digits past a sign or the
/// `#`) and what it says now (a number in its unit's base unit).
#[derive(Clone, Debug, PartialEq)]
pub struct LiteralEdit {
    pub line: u32,
    pub col: u32,
    pub value: LiteralValue,
}

/// A piece of Splash text: a number or colour literal (with its value), or
/// anything else, compared as written.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    Literal { at: usize, text: &'a str, value: LiteralValue },
}

/// The literals an edit from `old` to `new` changed, when that is all it
/// changed: the same text apart from number and colour literals (a sign,
/// a unit suffix, a word or a bracket that changed is a structural edit:
/// `None`). Every site is `old`'s, the text the running program was loaded
/// from, so a literal that grows does not move the others. Strings and
/// comments are text (a number in them is not a literal).
pub fn literal_edits(old: &str, new: &str) -> Option<Vec<LiteralEdit>> {
    let a = pieces(old);
    let b = pieces(new);
    if a.len() != b.len() {
        return None;
    }
    let mut out = Vec::new();
    for (x, y) in a.iter().zip(&b) {
        match (x, y) {
            (Piece::Text(p), Piece::Text(q)) if p == q => {}
            (Piece::Literal { at, text: p, .. }, Piece::Literal { text: q, value, .. }) => {
                if p != q {
                    let (line, col) = row_col(old, *at);
                    out.push(LiteralEdit { line, col, value: *value });
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Splits Splash text into literals and the text between them (strings,
/// raw strings and comments skipped whole). Adjacent text is one piece.
fn pieces(src: &str) -> Vec<Piece<'_>> {
    let b = src.as_bytes();
    let mut out: Vec<Piece> = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    while i < b.len() {
        let c = b[i];
        // Comments.
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
            continue;
        }
        // Raw strings r"…", r#"…"#.
        if c == b'r' && (i == 0 || !ident(b[i - 1])) {
            let mut j = i + 1;
            while j < b.len() && b[j] == b'#' {
                j += 1;
            }
            if b.get(j) == Some(&b'"') {
                let hashes = j - i - 1;
                let mut k = j + 1;
                loop {
                    if k >= b.len() {
                        break;
                    }
                    if b[k] == b'"' && b[k + 1..].iter().take(hashes).filter(|&&h| h == b'#').count() == hashes && b.len() >= k + 1 + hashes {
                        k += 1 + hashes;
                        break;
                    }
                    k += 1;
                }
                i = k.min(b.len());
                continue;
            }
        }
        if c == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            continue;
        }
        if ident(c) && !c.is_ascii_digit() {
            while i < b.len() && ident(b[i]) {
                i += 1;
            }
            continue;
        }
        // A colour: #rgb, #rgba, #rrggbb, #rrggbbaa.
        if c == b'#' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            let digits = &src[i + 1..j];
            if !(j < b.len() && ident(b[j])) {
                if let Some(rgba) = hex_color(digits) {
                    if text_start < i {
                        out.push(Piece::Text(&src[text_start..i]));
                    }
                    out.push(Piece::Literal { at: i, text: &src[i..j], value: LiteralValue::Color(rgba) });
                    i = j;
                    text_start = j;
                    continue;
                }
            }
            i = j.max(i + 1);
            continue;
        }
        // A number: digits, a fraction, an exponent, a unit suffix.
        if c.is_ascii_digit() && (i == 0 || !(ident(b[i - 1]) || b[i - 1] == b'.')) {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j < b.len() && b[j] == b'.' && b.get(j + 1).is_none_or(|d| d.is_ascii_digit() || !ident(*d)) {
                j += 1;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
            }
            if j < b.len() && (b[j] == b'e' || b[j] == b'E') {
                let mut k = j + 1;
                if k < b.len() && (b[k] == b'+' || b[k] == b'-') {
                    k += 1;
                }
                if k < b.len() && b[k].is_ascii_digit() {
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    j = k;
                }
            }
            let digits_end = j;
            while j < b.len() && ident(b[j]) {
                j += 1;
            }
            let suffix = &src[digits_end..j];
            let Ok(v) = src[i..digits_end].parse::<f64>() else {
                i = j;
                continue;
            };
            let value = if suffix.is_empty() {
                Some(v)
            } else {
                crate::tokenizer::ScriptUnit::parse(suffix).map(|u| u.to_base(v))
            };
            if let Some(v) = value {
                if text_start < i {
                    out.push(Piece::Text(&src[text_start..i]));
                }
                // The unit is part of what it says: a changed unit changes
                // the text piece after the literal.
                out.push(Piece::Literal { at: i, text: &src[i..digits_end], value: LiteralValue::Number(v) });
                text_start = digits_end;
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if text_start < src.len() {
        out.push(Piece::Text(&src[text_start..]));
    }
    out
}

/// A colour's hex digits (3, 4, 6 or 8) as its channels.
fn hex_color(d: &str) -> Option<[f32; 4]> {
    let n = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);
    let dup = |c: char| format!("{c}{c}");
    let c: Vec<char> = d.chars().collect();
    match c.len() {
        3 | 4 => {
            let ch = |k: usize| n(&dup(c[k]));
            Some([ch(0)?, ch(1)?, ch(2)?, if c.len() == 4 { ch(3)? } else { 1.0 }])
        }
        6 | 8 => Some([n(&d[0..2])?, n(&d[2..4])?, n(&d[4..6])?, if c.len() == 8 { n(&d[6..8])? } else { 1.0 }]),
        _ => None,
    }
}
