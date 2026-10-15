//! Export to rustc (plans/relax-borrowck.md section 6): code accepted only with field views is
//! rewritten mechanically into a form rustc accepts, driven by the checker's findings. The
//! prototype rewrite moves the field-level borrow out of the struct for the statement and puts
//! it back (`let mut __fv = mem::take(&mut P); <stmt with P -> __fv>; P = __fv;`), which is
//! sound exactly when the other side's field view does not touch P -- what the checker proved.
//! A borrow bound by `let r = &mut P;` and used past the conflicting statement is instead
//! borrowed again right after that statement (`let r = &mut P;` shadowing it): sound exactly
//! when the statement does not touch P (the checker proved it) and P is a plain place path
//! (fields and derefs of a local, re-evaluated without side effects). Other shapes (a field
//! without Default in a single statement, borrows used in the conflicting statement itself) are
//! left for the "split into a free fn over the disjoint fields" rewrite: reported, not
//! rewritten.

use super::bir::Span;
use crate::program::Program;

/// One conflict accepted only with field views.
#[derive(Clone)]
pub struct FvRecord {
    pub file: u32,
    /// the field-level borrow (or access) that is moved out / borrowed again
    pub span: Span,
    /// its place as source text
    pub place: String,
    /// its type implements Default
    pub default: bool,
    /// the conflicting access (the call fed the whole value)
    pub access: Span,
    /// extended two-phase borrows: `span` is the call argument holding the access, these are
    /// the arguments before it (the reserved one among them)
    pub hoist: Option<Vec<Span>>,
}

pub struct Rewrite {
    pub file: u32,
    pub new_src: String,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
}

/// Rewrites per file for the given field-view-only records.
pub fn rewrite(prog: &Program, records: &[FvRecord]) -> Vec<Rewrite> {
    let mut files: Vec<u32> = records.iter().map(|r| r.file).collect();
    files.sort();
    files.dedup();
    let mut out = Vec::new();
    for f in files {
        let pf = &prog.files[f as usize];
        let src = String::from_utf8_lossy(&pf.src[..pf.src_len as usize]).into_owned();
        let mut recs: Vec<&FvRecord> = records.iter().filter(|r| r.file == f).collect();
        // last first: earlier offsets stay valid (both rewrites only change text at or after
        // the borrow)
        recs.sort_by_key(|r| (std::cmp::Reverse(r.span.lo), std::cmp::Reverse(r.access.lo)));
        recs.dedup_by_key(|r| (r.span.lo, r.access.lo));
        let mut text = src.clone();
        let mut applied = Vec::new();
        let mut skipped = Vec::new();
        let mut n = 0;
        for r in recs {
            let (sp, place, dflt) = (&r.span, &r.place, &r.default);
            let (l, c) = pf.line_col(sp.lo);
            let at = format!("{}:{}:{}", prog.file_paths[f as usize], l, c);
            if let Some(prev) = &r.hoist {
                match hoist_arg(&text, sp.lo as usize, sp.hi as usize, prev, n) {
                    Some(t) => {
                        text = t;
                        applied.push(format!("{}: the argument is evaluated into a local before the statement", at));
                        n += 1;
                    }
                    None => skipped.push(format!("{}: the argument cannot be evaluated before the call (earlier arguments are not plain places)", at)),
                }
                continue;
            }
            if sp.lo >= pf.src_len || place.is_empty() {
                skipped.push(format!("{}: inside a macro expansion", at));
                continue;
            }
            if std::env::var_os("MAKEPAD_RUN_FV_EXPORT_DEBUG").is_some() {
                eprintln!("fv-export: {} place `{}` borrow {}..{} access {}..{}", at, place, sp.lo, sp.hi, r.access.lo, r.access.hi);
            }
            if let Some(t) = reborrow_after(&text, sp.lo as usize, place, r.access.lo as usize) {
                text = t;
                applied.push(format!("{}: `{}` borrowed again after the conflicting statement", at, place));
                continue;
            }
            if !*dflt {
                skipped.push(format!("{}: `{}` has no Default (needs the split-into-free-fn rewrite)", at, place));
                continue;
            }
            match rewrite_stmt(&text, sp.lo as usize, place, n) {
                Some(t) => {
                    text = t;
                    applied.push(format!("{}: `{}` moved out around the statement (mem::take)", at, place));
                    n += 1;
                }
                None => skipped.push(format!("{}: the borrow of `{}` is neither a single statement's nor a `let` re-borrowable after the conflict", at, place)),
            }
        }
        out.push(Rewrite { file: f, new_src: text, applied, skipped });
    }
    out
}

/// The statement around `pos`: from its line's first non-blank to the `;` that ends it at
/// bracket depth 0. `place` must occur in it exactly once, at `pos`.
fn rewrite_stmt(text: &str, mut pos: usize, place: &str, n: usize) -> Option<String> {
    let b = text.as_bytes();
    // the span may be the borrow expression `&P` (the borrow becomes `&__fv`)
    if pos < b.len() && b[pos] == b'&' {
        let r = &text[pos + 1..];
        let r2 = r.strip_prefix("mut ").unwrap_or(r).trim_start();
        pos = text.len() - r2.len();
    }
    if pos >= b.len() || !text[pos..].starts_with(place) {
        return None;
    }
    let line0 = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line0..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let start = line0 + indent.len();
    // the statement must start on this line (no partial expressions above)
    let mut depth: i32 = 0;
    let mut i = start;
    let mut end = None;
    let mut in_str = false;
    while i < b.len() {
        let ch = b[i];
        if in_str {
            if ch == b'\\' {
                i += 2;
                continue;
            }
            if ch == b'"' {
                in_str = false;
            }
        } else {
            match ch {
                b'"' => in_str = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                b';' if depth == 0 => {
                    end = Some(i);
                    break;
                }
                _ => {}
            }
        }
        i += 1;
    }
    let end = end?;
    let stmt = &text[start..end];
    // a borrow bound by `let` outlives the statement: putting the field back right after it
    // would move it while borrowed
    if stmt.starts_with("let ") {
        return None;
    }
    if stmt.matches(place).count() != 1 || !stmt.starts_with(place) && pos < start {
        return None;
    }
    let tmp = format!("__fv{}", n);
    let new_stmt = format!("{}{}{}", &text[start..pos], tmp, &text[pos + place.len()..end]);
    let repl = format!(
        "let mut {t} = std::mem::take(&mut {p});\n{ind}{s};\n{ind}{p} = {t}",
        t = tmp,
        p = place,
        ind = indent,
        s = new_stmt
    );
    Some(format!("{}{}{}", &text[..start], repl, &text[end..]))
}

/// The end (index of the `;`) of the statement starting at `start`, at bracket depth 0.
fn stmt_end(b: &[u8], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut i = start;
    let mut in_str = false;
    while i < b.len() {
        let ch = b[i];
        if in_str {
            if ch == b'\\' {
                i += 2;
                continue;
            }
            if ch == b'"' {
                in_str = false;
            }
        } else {
            match ch {
                b'"' => in_str = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                b';' if depth == 0 => return Some(i),
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn is_word_at(text: &str, i: usize, w: &str) -> bool {
    let b = text.as_bytes();
    let ok = |c: u8| !(c.is_ascii_alphanumeric() || c == b'_');
    text[i..].starts_with(w) && (i == 0 || ok(b[i - 1])) && b.get(i + w.len()).map_or(true, |c| ok(*c))
}

fn has_word(text: &str, w: &str) -> bool {
    text.match_indices(w).any(|(i, _)| is_word_at(text, i, w))
}

/// `let r = &[mut] P;` ... `<stmt with the conflict>;` ... uses of r: borrow P again right
/// after the conflicting statement, shadowing r. Both statements must be in the same block
/// (the conflicting one reached from the `let` without leaving or entering a block), the
/// conflicting statement must not name r, and P must be a plain place path.
fn reborrow_after(text: &str, mut pos: usize, place: &str, access: usize) -> Option<String> {
    let b = text.as_bytes();
    // the span may be the borrow expression `&mut P`
    if pos < b.len() && b[pos] == b'&' {
        let r = &text[pos + 1..];
        let r2 = r.strip_prefix("mut ").unwrap_or(r).trim_start();
        pos = text.len() - r2.len();
    }
    if pos >= b.len() || access <= pos || !text[pos..].starts_with(place) {
        return None;
    }
    let plain = |t: &str| t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'*' || c == b' ' || c == b'[' || c == b']');
    if !plain(place) {
        return None;
    }
    let line0 = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line0..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let start = line0 + indent.len();
    let end = stmt_end(b, start)?;
    let stmt = text[start..end].trim_end();
    // `let NAME = &P` / `let NAME = &mut P`
    let rest = stmt.strip_prefix("let ")?.trim_start();
    let name_len = rest.bytes().take_while(|c| c.is_ascii_alphanumeric() || *c == b'_').count();
    let name = &rest[..name_len];
    if name.is_empty() || name == "mut" {
        return None;
    }
    let rhs = rest[name_len..].trim_start().strip_prefix('=')?.trim_start();
    let (amp, p) = if let Some(p) = rhs.strip_prefix("&mut ") { ("&mut ", p) } else { ("&", rhs.strip_prefix('&')?) };
    // P itself, or an element / field of it (`&self.items[i]`): evaluated again unchanged,
    // since the conflicting statement does not touch P
    let p = p.trim();
    if !p.starts_with(place) || !plain(p) || !(p.len() == place.len() || matches!(p.as_bytes()[place.len()], b'[' | b'.')) {
        return None;
    }
    // the conflicting statement: same block, after the `let`
    let mut depth: i32 = 0;
    let mut i = end + 1;
    let mut in_str = false;
    while i < access {
        let ch = b[i];
        if in_str {
            if ch == b'\\' {
                i += 2;
                continue;
            }
            if ch == b'"' {
                in_str = false;
            }
        } else {
            match ch {
                b'"' => in_str = true,
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => {
                    depth -= 1;
                    if depth < 0 {
                        return None;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    let a0 = text[..access].rfind('\n').map_or(0, |i| i + 1);
    let aind: String = text[a0..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let astart = a0 + aind.len();
    if astart > access || text[astart..access].contains(';') {
        return None;
    }
    let aend = stmt_end(b, astart)?;
    if has_word(&text[astart..aend], name) {
        return None;
    }
    // an index the conflicting statement could change is not evaluated again
    for part in p.split('[').skip(1) {
        let idx = part.split(']').next().unwrap_or("").trim();
        if !idx.bytes().all(|c| c.is_ascii_digit()) && has_word(&text[astart..aend], idx) {
            return None;
        }
    }
    Some(format!("{}\n{}let {} = {}{};{}", &text[..aend + 1], aind, name, amp, p, &text[aend + 1..]))
}

/// `f(vm, g(vm))` -> `let __fvN = g(vm);` before the statement and `f(vm, __fvN)`: the
/// arguments before it must be plain places (evaluating them has no effect to reorder).
fn hoist_arg(text: &str, lo: usize, hi: usize, prev: &[Span], n: usize) -> Option<String> {
    let b = text.as_bytes();
    if hi > b.len() || lo >= hi {
        return None;
    }
    for p in prev {
        let t = text.get(p.lo as usize..p.hi as usize)?.trim();
        let t = t.strip_prefix("&mut ").or_else(|| t.strip_prefix('&')).unwrap_or(t).trim();
        if t.is_empty() || !t.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c == b'*') {
            return None;
        }
    }
    // the enclosing statement: back to the `;` / `{` / `}` before it (strings in between are
    // not expected in a call's leading arguments)
    let mut depth: i32 = 0;
    let mut i = lo;
    let mut start = 0;
    while i > 0 {
        i -= 1;
        match b[i] {
            b')' | b']' => depth += 1,
            b'(' | b'[' => depth -= 1,
            b'{' if depth == 0 => {
                start = i + 1;
                break;
            }
            b'}' | b';' if depth == 0 => {
                start = i + 1;
                break;
            }
            b'"' => return None,
            _ => {}
        }
        if depth < 0 {
            // inside the call's parentheses: keep going out
            depth = 0;
        }
    }
    while start < lo && (b[start] == b' ' || b[start] == b'\t' || b[start] == b'\n' || b[start] == b'\r') {
        start += 1;
    }
    let end = stmt_end(b, start)?;
    if end < hi {
        return None;
    }
    // evaluated unconditionally and once before the call: no closure, branch, loop or
    // short-circuit between the statement's start and the argument
    let head = &text[start..lo];
    if head.contains('|') || head.contains("=>") || head.contains('?') || ["if", "while", "for", "loop", "match", "else", "return", "break"].iter().any(|w| has_word(head, w)) {
        return None;
    }
    let line0 = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let own_line = text[line0..start].bytes().all(|c| c == b' ' || c == b'\t');
    let sep = if own_line { format!("\n{}", &text[line0..start]) } else { " ".to_string() };
    let arg = &text[lo..hi];
    let tmp = format!("__fv{}", n);
    Some(format!("{}let {} = {};{}{}{}{}", &text[..start], tmp, arg, sep, &text[start..lo], tmp, &text[hi..]))
}
