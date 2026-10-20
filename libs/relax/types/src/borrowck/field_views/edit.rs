//! The export of field-view-only acceptances to rustc Rust (plans/relax-borrowck.md section 6) as
//! typed edits. Each rewrite is derived from AST and MIR facts (the statement containing the
//! borrow, the call argument to hoist, the place expression, the `let` that bound the borrow);
//! nothing searches or parses source text. Edits are (file, byte range, replacement) triples at
//! positions the AST and token table give; `apply` splices them.
//!
//! Rewrites (sound exactly when the other side's field view does not touch the place, which the
//! checker proved):
//!  - hoist: `f(a, g(a))` -> `let __fvN = g(a);` before the statement, `f(a, __fvN)`; sound when the
//!    arguments before it are plain places and nothing conditional lies between statement and argument.
//!  - reborrow: `let r = &mut P;` .. `<stmt touching other fields>;` .. uses of `r`: `let r = &mut P;`
//!    again right after that statement.
//!  - take around: `let mut __fvN = mem::take(&mut P); <stmt with P -> __fvN>; P = __fvN;`, when
//!    `P`'s type is `Default` and the statement is not a `let`.
//! Other shapes are reported as skipped (the "split into a free fn over the disjoint fields" rewrite).

use super::FvRecordRaw;
use crate::ast::{Ast, BlockId, ExprId, ExprKind, Pat, Stmt, UnOp};
use crate::mir::Span;
use crate::program::Program;
use crate::relaxed::Edit;

#[derive(Debug)]
pub struct PlannedRewrite {
    pub file: u32,
    pub edits: Vec<Edit>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
}

/// One statement of a block with its extent (the `let` keyword / expression start through the `;`).
#[derive(Clone, Copy, Debug)]
struct StmtRange {
    block: BlockId,
    index: usize,
    lo: u32,
    hi: u32,
}

struct FileView<'a> {
    prog: &'a Program,
    file: u32,
}
impl<'a> FileView<'a> {
    fn pf(&self) -> &'a crate::parser::ParsedFile { &self.prog.files[self.file as usize] }
    fn ast(&self) -> &'a Ast { &self.pf().ast }
    fn text(&self, lo: u32, hi: u32) -> String { String::from_utf8_lossy(self.pf().bytes(lo, hi)).into_owned() }

    /// Index of the first token starting at or after `pos` (the token table is ordered by position).
    fn tok_from(&self, pos: u32) -> u32 {
        let pf = self.pf();
        let eof = |i: u32| pf.tok(i).kind == crate::lexer::T::Eof;
        let mut hi = 1u32;
        while !eof(hi) && pf.tok(hi).lo < pos { hi = hi.saturating_mul(2); }
        let (mut lo, mut hi) = (hi / 2, hi);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if eof(mid) || pf.tok(mid).lo >= pos { hi = mid } else { lo = mid + 1 }
        }
        lo
    }
    /// The `let` keyword before a let statement whose first known position is `at`.
    fn let_keyword_before(&self, at: u32) -> Option<u32> {
        let mut i = self.tok_from(at);
        while i > 0 {
            i -= 1;
            if self.pf().tok_text(i) == "let" { return Some(self.pf().tok(i).lo); }
        }
        None
    }
    /// The `;` token starting at or after `end`, when it is the next token.
    fn semi_after(&self, end: u32) -> Option<u32> {
        let i = self.tok_from(end);
        let t = self.pf().tok(i);
        (self.pf().tok_text(i) == ";").then_some(t.hi)
    }

    fn stmt_ranges(&self, block: BlockId) -> Vec<StmtRange> {
        let ast = self.ast();
        let mut out = Vec::new();
        for (index, stmt) in ast.stmts(block).iter().enumerate() {
            let range = match *stmt {
                Stmt::Expr(e, semi) => {
                    let ex = ast.expr(e);
                    let hi = if semi { self.semi_after(ex.hi) } else { Some(ex.hi) };
                    hi.map(|hi| (ex.lo, hi))
                }
                Stmt::Let(row) => {
                    let l = ast.row(row);
                    match (l.init, ast.pat(l.pat)) {
                        (Some(init), Pat::Ident { name, .. }) => {
                            let ex = ast.expr(init);
                            self.let_keyword_before(name.lo).and_then(|lo| self.semi_after(ex.hi).map(|hi| (lo, hi)))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((lo, hi)) = range { out.push(StmtRange { block, index, lo, hi }); }
        }
        out
    }

    /// The innermost block whose statements contain `span`, with the statement that does.
    fn enclosing_stmt(&self, span: Span) -> Option<StmtRange> {
        let ast = self.ast();
        let mut best: Option<(u32, StmtRange)> = None;
        for id in self.blocks() {
            let b = ast.block(id);
            if !(b.lo <= span.lo && span.hi <= b.hi) { continue; }
            for r in self.stmt_ranges(id) {
                if r.lo <= span.lo && span.hi <= r.hi && best.map_or(true, |(w, _)| b.hi - b.lo < w) {
                    best = Some((b.hi - b.lo, r));
                }
            }
        }
        best.map(|(_, r)| r)
    }

    /// Every expression of the file: its tree and its lazily parsed fn bodies.
    fn exprs(&self) -> impl Iterator<Item = (ExprId, crate::ast::Expr)> + '_ {
        let ast = self.ast();
        let trees = std::iter::once(ast).chain(ast.part.get().map(|p| &**p));
        trees.flat_map(|t| (0..t.exprs.len()).map(move |i| { let id = ExprId(t.base[0] + i as u32); (id, *t.expr(id)) }))
    }
    /// Every block of the file (its tree and its lazily parsed part).
    fn blocks(&self) -> impl Iterator<Item = BlockId> + '_ {
        let ast = self.ast();
        let trees = std::iter::once(ast).chain(ast.part.get().map(|p| &**p));
        trees.flat_map(|t| (0..t.blocks.len()).map(move |i| BlockId(t.base[3] + i as u32)))
    }
    /// The expression whose extent is exactly `span`.
    fn expr_at(&self, span: Span) -> Option<(ExprId, crate::ast::Expr)> {
        self.exprs().find(|(_, e)| e.lo == span.lo && e.hi == span.hi)
    }
    /// A place expression with no side effects to reorder: paths, fields, derefs, literal indices.
    fn is_plain(&self, e: ExprId) -> bool {
        let ast = self.ast();
        match ast.expr(e).kind {
            ExprKind::Path(_) => true,
            ExprKind::Paren(x) | ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Unary(UnOp::Deref, x) => self.is_plain(x),
            ExprKind::Index(x, i) => self.is_plain(x) && matches!(ast.expr(i).kind, ExprKind::Lit(_) | ExprKind::Path(_)),
            ExprKind::AddrOf(false, _, x) => self.is_plain(x),
            _ => false,
        }
    }
    /// Expressions strictly containing `span` that lie inside `outer`, outermost last.
    fn ancestors(&self, span: Span, outer: (u32, u32)) -> Vec<crate::ast::Expr> {
        self.exprs().map(|(_, e)| e)
            .filter(|e| e.lo <= span.lo && span.hi <= e.hi && (e.lo, e.hi) != (span.lo, span.hi) && outer.0 <= e.lo && e.hi <= outer.1)
            .collect()
    }
    fn indent_at(&self, pos: u32) -> String {
        let (_, col) = self.pf().line_col(pos);
        let line_start = pos - (col as u32 - 1);
        let lead = self.pf().bytes(line_start, pos);
        if lead.iter().all(|c| *c == b' ' || *c == b'\t') { String::from_utf8_lossy(lead).into_owned() } else { String::new() }
    }
    /// Whether `name` is used as a single-segment path inside `range`.
    fn mentions(&self, range: (u32, u32), name: &str) -> bool {
        let ast = self.ast();
        self.exprs().any(|(_, e)| {
            range.0 <= e.lo && e.hi <= range.1
                && matches!(e.kind, ExprKind::Path(p) if { let s = ast.segs(p.segs); s.len() == 1 && self.pf().text(s[0].name) == name })
        })
    }
}

/// Plan the rewrites of `records` (one `PlannedRewrite` per file).
pub fn plan(prog: &Program, records: &[FvRecordRaw]) -> Vec<PlannedRewrite> {
    let mut files: Vec<u32> = records.iter().map(|r| r.span.file).collect();
    files.sort();
    files.dedup();
    let mut out = Vec::new();
    for file in files {
        let view = FileView { prog, file };
        let mut recs: Vec<&FvRecordRaw> = records.iter().filter(|r| r.span.file == file).collect();
        recs.sort_by_key(|r| (r.span.lo, r.access.lo));
        recs.dedup_by_key(|r| (r.span.lo, r.access.lo));
        let mut rw = PlannedRewrite { file, edits: Vec::new(), applied: Vec::new(), skipped: Vec::new() };
        let mut n = 0;
        for r in recs {
            let (l, c) = view.pf().line_col(r.span.lo);
            let at = format!("{}:{}:{}", prog.file_paths[file as usize], l, c);
            match rewrite_one(&view, r, n) {
                Ok((edits, what)) => {
                    if edits.iter().any(|e| rw.edits.iter().any(|o| e.lo < o.hi.max(o.lo + 1) && o.lo < e.hi.max(e.lo + 1) || (e.lo == o.lo && e.hi == o.hi))) {
                        rw.skipped.push(format!("{at}: overlaps another rewrite"));
                    } else {
                        rw.edits.extend(edits);
                        rw.applied.push(format!("{at}: {what}"));
                        n += 1;
                    }
                }
                Err(why) => rw.skipped.push(format!("{at}: {why}")),
            }
        }
        out.push(rw);
    }
    out
}

fn rewrite_one(v: &FileView<'_>, r: &FvRecordRaw, n: usize) -> Result<(Vec<Edit>, String), &'static str> {
    let ast = v.ast();
    if r.span.hi > v.pf().src_len { return Err("inside a macro expansion"); }
    let stmt = v.enclosing_stmt(r.span).ok_or("the borrow is not inside a plain statement")?;
    let tmp = format!("__fv{n}");
    let indent = v.indent_at(stmt.lo);

    // extended two-phase borrows: hoist the argument that holds the access
    if let Some(prev) = &r.hoist {
        let (_, arg) = v.expr_at(r.span).ok_or("the argument is not an expression")?;
        for p in prev {
            let (e, _) = v.expr_at(*p).ok_or("an earlier argument is not an expression")?;
            if !v.is_plain(e) { return Err("the argument cannot be evaluated before the call (earlier arguments are not plain places)"); }
        }
        // evaluated unconditionally and once before the call: only calls, parentheses and
        // reference/field forms between the statement and the argument
        let allowed = |e: &crate::ast::Expr| matches!(e.kind, ExprKind::Call(..) | ExprKind::MethodCall(..) | ExprKind::Paren(_) | ExprKind::AddrOf(..)
            | ExprKind::Field(..) | ExprKind::TupleField(..) | ExprKind::Unary(UnOp::Deref, _) | ExprKind::Assign(..));
        if !v.ancestors(r.span, (stmt.lo, stmt.hi)).iter().all(allowed) {
            return Err("the argument is evaluated conditionally (closure, branch, loop or short-circuit around it)");
        }
        let text = v.text(arg.lo, arg.hi);
        return Ok((vec![
            Edit { lo: stmt.lo, hi: stmt.lo, text: format!("let {tmp} = {text};\n{indent}") },
            Edit { lo: arg.lo, hi: arg.hi, text: tmp },
        ], "the argument is evaluated into a local before the statement".to_string()));
    }

    // the field-level borrow `&P` / `&mut P`, or the place `P` itself
    let (borrow_id, borrow) = v.expr_at(r.span).ok_or("the borrow is not an expression")?;
    let (place_id, place) = match borrow.kind { ExprKind::AddrOf(false, _, x) => (x, *ast.expr(x)), _ => (borrow_id, borrow) };
    if !v.is_plain(place_id) { return Err("the place is not a plain place path"); }
    let place_text = v.text(place.lo, place.hi);

    // `let r = &[mut] P;` .. <conflicting statement> .. uses of `r`: borrow again after the conflict
    let stmts = ast.stmts(stmt.block);
    if let Stmt::Let(row) = stmts[stmt.index] {
        let l = ast.row(row);
        if let (Some(init), Pat::Ident { by_ref: false, mutbl: false, name, sub: None }) = (l.init, ast.pat(l.pat)) {
            let init_e = ast.expr(init);
            if init_e.lo <= r.span.lo && r.span.hi <= init_e.hi && matches!(init_e.kind, ExprKind::AddrOf(false, _, x) if v.is_plain(x)) {
                let bound = v.pf().text(*name).to_string();
                let access = v.stmt_ranges(stmt.block).into_iter().find(|s| s.index > stmt.index && s.lo <= r.access.lo && r.access.hi <= s.hi)
                    .ok_or("the conflicting statement is not later in the same block")?;
                if v.mentions((access.lo, access.hi), &bound) { return Err("the conflicting statement uses the borrow"); }
                let init_text = v.text(init_e.lo, init_e.hi);
                let indent = v.indent_at(access.lo);
                return Ok((vec![Edit { lo: access.hi, hi: access.hi, text: format!("\n{indent}let {bound} = {init_text};") }],
                    format!("`{place_text}` borrowed again after the conflicting statement")));
            }
        }
        return Err("the borrow of a field is neither a single statement's nor a `let` re-borrowable after the conflict");
    }
    if !r.default { return Err("the place has no Default (needs the split-into-free-fn rewrite)"); }
    // the place occurs once in the statement, inside the borrow
    let occurrences = v.exprs().filter(|(id, e)| stmt.lo <= e.lo && e.hi <= stmt.hi && !matches!(e.kind, ExprKind::Paren(_)) && v.is_plain(*id) && v.same_place(*id, place_id)).count();
    if occurrences != 1 { return Err("the place occurs more than once in the statement"); }
    Ok((vec![
        Edit { lo: stmt.lo, hi: stmt.lo, text: format!("let mut {tmp} = std::mem::take(&mut {place_text});\n{indent}") },
        Edit { lo: place.lo, hi: place.hi, text: tmp.clone() },
        Edit { lo: stmt.hi, hi: stmt.hi, text: format!("\n{indent}{place_text} = {tmp};") },
    ], format!("`{place_text}` moved out around the statement (mem::take)")))
}

// ---------------------------------------------------------------------------------------------
// Polonius (A2) and disjoint-index (D3) acceptances: relaxed notes with structured edits.

/// The relaxed notes of `records` (one per accepted error, `--strict-report` lists them), each with
/// the edits of its rewrite when one is known and exact; otherwise the note says why there is none.
/// Records that share a loan share one rewrite (`apply` takes identical edits once).
pub fn plan_relaxed(prog: &Program, records: &[super::RelaxedRecord]) -> Vec<crate::relaxed::Note> {
    use super::RelaxedRecord;
    let mut out = Vec::new();
    for r in records {
        match r {
            RelaxedRecord::Polonius(p) => {
                let at = |span: Span| {
                    let (l, c) = prog.files[span.file as usize].line_col(span.lo);
                    format!("{}:{}:{}", prog.file_paths[span.file as usize], l, c)
                };
                let view = FileView { prog, file: p.loan.file };
                let planned = match p.lookup {
                    Some(lookup) if lookup.call.file == p.loan.file => relookup(&view, lookup),
                    Some(_) => Err("the lookup is in another file"),
                    None => Err("the borrow is not a std map lookup with a std-compared key (no exact rewrite: entry API or a split borrow would change the code's structure)"),
                };
                let (fix, how) = match planned {
                    Ok((edits, what)) => (Some((prog.file_paths[p.loan.file as usize].clone(), edits)), format!("will write: {what}")),
                    Err(why) => (None, format!("no known rewrite: {why}")),
                };
                out.push(crate::relaxed::Note {
                    at: at(p.span), feature: "polonius", code: p.code,
                    msg: format!("accepted under Polonius; stable rustc rejects it (the borrow at {} is not live on this path); {how}", at(p.loan)),
                    fix,
                });
            }
            RelaxedRecord::Disjoint(d) => {
                let at = |span: Span| {
                    let (l, c) = prog.files[span.file as usize].line_col(span.lo);
                    format!("{}:{}:{}", prog.file_paths[span.file as usize], l, c)
                };
                let view = FileView { prog, file: d.second.file };
                let planned = if d.first.file != d.second.file { Err("the borrows are in different files") } else { disjoint_rewrite(&view, d) };
                let (fix, how) = match planned {
                    Ok((edits, what)) => (Some((prog.file_paths[d.second.file as usize].clone(), edits)), format!("will write: {what}")),
                    Err(why) => (None, format!("no known rewrite: {why}")),
                };
                let why = if d.proven { "the indices are distinct" } else { "the indices are compared when this runs (an overlap panics, as `get_disjoint_mut(..).unwrap()` does)" };
                out.push(crate::relaxed::Note {
                    at: at(d.second), feature: "disjoint-index", code: d.code,
                    msg: format!("accepted with disjoint indices; stable rustc rejects it (`{}` is borrowed at {}); {why}; {how}", view.text(d.first.lo, d.first.hi), at(d.first)),
                    fix,
                });
            }
        }
    }
    out
}

/// The `get_disjoint_mut` rewrite of two index borrows accepted as disjoint:
/// `let a = &mut V[I]; let b = &mut V[J];` (two consecutive statements) ->
/// `let [a, b] = V.get_disjoint_mut([I, J]).unwrap();`. Exact when `V`, `I` and `J` are side-effect-free
/// (places, literals, `+` / `-` of them): the same elements are borrowed and an overlap panics, which is
/// what the checked lowering does (an index out of bounds panics in both, with another message).
fn disjoint_rewrite(v: &FileView<'_>, d: &super::DisjointRecord) -> Result<(Vec<Edit>, String), &'static str> {
    let ast = v.ast();
    if !d.both_mut { return Err("one of the borrows is shared (`get_disjoint_mut` makes both mutable)"); }
    if d.second.hi > v.pf().src_len || d.first.hi > v.pf().src_len { return Err("inside a macro expansion"); }
    // `let name = &mut base[index];` holding the index expression at `span`
    let borrow_let = |span: Span| -> Result<(StmtRange, String, ExprId, ExprId), &'static str> {
        let (index_id, index) = v.exprs().find(|(_, e)| e.lo == span.lo && e.hi == span.hi && matches!(e.kind, ExprKind::Index(..)))
            .ok_or("the borrow is not an index expression")?;
        let ExprKind::Index(base, i) = index.kind else { unreachable!() };
        let stmt = v.enclosing_stmt(span).ok_or("the borrow is not inside a plain statement")?;
        let Stmt::Let(row) = ast.stmts(stmt.block)[stmt.index] else { return Err("the borrow is not a `let`") };
        let l = ast.row(row);
        let (Some(init), Pat::Ident { by_ref: false, mutbl, name, sub: None }, None, None) = (l.init, *ast.pat(l.pat), l.ty, l.else_) else {
            return Err("the `let` is not `let <name> = ..;`");
        };
        if !matches!(ast.expr(v.unparen(init)).kind, ExprKind::AddrOf(false, true, x) if v.unparen(x) == index_id) { return Err("the `let` does not bind `&mut base[index]`"); }
        let name = format!("{}{}", if mutbl { "mut " } else { "" }, v.pf().text(name));
        Ok((stmt, name, base, i))
    };
    let (s1, a, base1, i1) = borrow_let(d.first)?;
    let (s2, b, base2, i2) = borrow_let(d.second)?;
    if s1.block.0 != s2.block.0 || s2.index != s1.index + 1 { return Err("the two borrows are not consecutive `let`s"); }
    if !v.simple_place(base1) || !v.same_place(base1, base2) { return Err("the container is not the same plain place"); }
    if !v.simple_index(i1) || !v.simple_index(i2) { return Err("an index is not a side-effect-free expression"); }
    let text = |e: ExprId| { let x = ast.expr(e); v.text(x.lo, x.hi) };
    let base = text(base1);
    let call = format!("let [{a}, {b}] = {base}.get_disjoint_mut([{}, {}]).unwrap();", text(i1), text(i2));
    Ok((vec![Edit { lo: s1.lo, hi: s2.hi, text: call }], format!("`{base}.get_disjoint_mut([..]).unwrap()` borrows both elements")))
}

/// The re-lookup rewrite of a std map lookup whose borrow Polonius ends on the miss path:
///  - `if let Some(v) = M.get_mut(K) { A } ..` -> `if M.contains_key(K) { let v = M.get_mut(K).unwrap(); A } ..`
///  - `match M.get_mut(K) { Some(v) => A, None => B }` -> `if M.contains_key(K) { let v = M.get_mut(K).unwrap(); A } else { B }`
/// (`get` likewise). Exact when `M` and `K` are side-effect-free places (the checker proved the map is
/// reached without an overloaded deref and the key compares by std's own impls, so the second lookup
/// finds the entry the first found); NLL then sees the borrow only on the hit path.
fn relookup(v: &FileView<'_>, lookup: super::Lookup) -> Result<(Vec<Edit>, String), &'static str> {
    let ast = v.ast();
    if lookup.call.hi > v.pf().src_len { return Err("inside a macro expansion"); }
    let (call_id, call) = v.exprs().find(|(_, e)| e.lo == lookup.call.lo && e.hi == lookup.call.hi && matches!(e.kind, ExprKind::MethodCall(..)))
        .ok_or("the lookup is not a method call expression")?;
    let ExprKind::MethodCall(recv, row) = call.kind else { return Err("the lookup is not a method call") };
    let row = ast.row(row);
    let args = ast.list(row.args);
    if args.len() != 1 || !v.simple_place(recv) || !v.simple_key(args[0]) { return Err("the map or the key is not a plain place"); }
    let method = if lookup.mutable { "get_mut" } else { "get" };
    if v.pf().text(row.name) != method { return Err("the lookup is written through another method"); }
    let (r, k) = (v.text(ast.expr(recv).lo, ast.expr(recv).hi), v.text(ast.expr(args[0]).lo, ast.expr(args[0]).hi));
    let contains = format!("{r}.contains_key({k})");
    let again = |name: &str, mutbl: bool| format!("let {}{name} = {r}.{method}({k}).unwrap();", if mutbl { "mut " } else { "" });
    // `Some(name)` / `Some(mut name)` with a by-value binding
    let some_binding = |pat: crate::ast::PatId| -> Option<(String, bool)> {
        let crate::ast::Pat::TupleStruct(path, ps) = *ast.pat(pat) else { return None };
        let segs = ast.segs(path.segs);
        if v.pf().text(segs.last()?.name) != "Some" { return None; }
        let [p] = ast.list(ps) else { return None };
        match *ast.pat(*p) {
            crate::ast::Pat::Ident { by_ref: false, mutbl, name, sub: None } => Some((v.pf().text(name).to_string(), mutbl)),
            _ => None,
        }
    };
    // `None` (an identifier pattern until resolution; on the miss path the scrutinee is `None` whether
    // it names the variant or binds), `Option::None`, or `_`
    let is_none = |pat: crate::ast::PatId| match *ast.pat(pat) {
        crate::ast::Pat::Wild => true,
        crate::ast::Pat::Ident { by_ref: false, mutbl: false, name, sub: None } => v.pf().text(name) == "None",
        crate::ast::Pat::Path(path) => ast.segs(path.segs).last().is_some_and(|s| v.pf().text(s.name) == "None"),
        _ => false,
    };
    for (id, e) in v.exprs() {
        match e.kind {
            // `if let Some(v) = <call> { .. }`: the `let` is the whole condition
            ExprKind::Let(pat, scrut) if v.unparen(scrut) == call_id => {
                let (name, mutbl) = some_binding(pat).ok_or("the pattern is not `Some(<binding>)`")?;
                let (_, then) = v.exprs().find_map(|(_, x)| match x.kind { ExprKind::If(c, t, _) if c == id => Some((c, t)), _ => None })
                    .ok_or("the `let` is not the whole condition of an `if`")?;
                let open = ast.block(then).lo;
                if v.pf().bytes(open, open + 1) != b"{" { return Err("the block does not start at its brace"); }
                // on its own line when the block's first statement is
                let first = v.tok_from(open + 1);
                let first_lo = v.pf().tok(first).lo;
                let lead = if v.pf().line_col(first_lo).0 > v.pf().line_col(open).0 { format!("\n{}", v.indent_at(first_lo)) } else { " ".to_string() };
                return Ok((vec![
                    Edit { lo: e.lo, hi: e.hi, text: contains.clone() },
                    Edit { lo: open + 1, hi: open + 1, text: format!("{lead}{}", again(&name, mutbl)) },
                ], format!("`{contains}` before a second `{method}` on the hit path (re-lookup)")));
            }
            ExprKind::Match(scrut, arms) if v.unparen(scrut) == call_id => {
                let arms = ast.list(arms);
                if arms.len() != 2 || arms.iter().any(|a| a.guard.is_some() || !ast.list(a.attrs).is_empty()) { return Err("the match is not two plain arms"); }
                let (hit, miss) = if some_binding(arms[0].pat).is_some() && is_none(arms[1].pat) { (&arms[0], &arms[1]) }
                    else if some_binding(arms[1].pat).is_some() && is_none(arms[0].pat) { (&arms[1], &arms[0]) }
                    else { return Err("the arms are not `Some(<binding>)` and `None`") };
                let (name, mutbl) = some_binding(hit.pat).unwrap();
                // the match is a whole expression (not the receiver of `.`, `?` or `[`)
                let next = v.tok_from(e.hi);
                if matches!(v.pf().tok_text(next), "." | "?" | "[") { return Err("the match is used as a receiver"); }
                let body = |a: &crate::ast::Arm| { let b = ast.expr(a.body); v.text(b.lo, b.hi) };
                // a block arm is the `else` block itself
                let miss_block = matches!(ast.expr(miss.body).kind, ExprKind::Block(_, l) if l.get().is_none());
                let otherwise = if miss_block { body(miss) } else { format!("{{ {} }}", body(miss)) };
                return Ok((vec![Edit { lo: e.lo, hi: e.hi,
                    text: format!("if {contains} {{ {} {} }} else {otherwise}", again(&name, mutbl), body(hit)) }],
                    format!("`{contains}` before a second `{method}` on the hit path (re-lookup)")));
            }
            _ => {}
        }
    }
    Err("the lookup is not the scrutinee of an `if let` or a `match`")
}

impl FileView<'_> {
    fn unparen(&self, mut e: ExprId) -> ExprId {
        while let ExprKind::Paren(x) = self.ast().expr(e).kind { e = x; }
        e
    }
    /// A place written as a path with fields (`map`, `self.cache.map`): evaluated again it is the
    /// same place, with no call (no index, no explicit deref).
    fn simple_place(&self, e: ExprId) -> bool {
        match self.ast().expr(e).kind {
            ExprKind::Path(p) => self.ast().qself(&p).is_none(),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) | ExprKind::Paren(x) => self.simple_place(x),
            _ => false,
        }
    }
    /// An index without side effects: a simple place, a literal, `+` / `-` of those.
    fn simple_index(&self, e: ExprId) -> bool {
        match self.ast().expr(e).kind {
            ExprKind::Lit(_) => true,
            ExprKind::Paren(x) => self.simple_index(x),
            ExprKind::Binary(crate::ast::BinOp::Add | crate::ast::BinOp::Sub, x, y) => self.simple_index(x) && self.simple_index(y),
            _ => self.simple_place(e),
        }
    }
    /// A lookup key: a simple place or a literal, possibly borrowed (`&k`, `&self.id`, `&3`, `"a"`).
    fn simple_key(&self, e: ExprId) -> bool {
        match self.ast().expr(e).kind {
            ExprKind::Lit(_) => true,
            ExprKind::AddrOf(false, false, x) | ExprKind::Paren(x) => self.simple_key(x),
            _ => self.simple_place(e),
        }
    }
}

impl FileView<'_> {
    /// Whether two plain place expressions denote the same place: the same shape over the same
    /// names (identifiers compare by name, as rustc's `Symbol`s do), field names, tuple indices and
    /// literal indices (compared by their token). Parentheses are transparent. No source text of
    /// a whole expression is compared.
    fn same_place(&self, a: ExprId, b: ExprId) -> bool {
        let ast = self.ast();
        let (ea, eb) = (ast.expr(a), ast.expr(b));
        match (ea.kind, eb.kind) {
            (ExprKind::Paren(x), _) => self.same_place(x, b),
            (_, ExprKind::Paren(y)) => self.same_place(a, y),
            (ExprKind::Path(p), ExprKind::Path(q)) => {
                let (sp, sq) = (ast.segs(p.segs), ast.segs(q.segs));
                sp.len() == sq.len() && sp.iter().zip(sq).all(|(x, y)| self.pf().text(x.name) == self.pf().text(y.name))
            }
            (ExprKind::Field(x, n), ExprKind::Field(y, m)) => self.pf().text(n) == self.pf().text(m) && self.same_place(x, y),
            (ExprKind::TupleField(x, i), ExprKind::TupleField(y, j)) => i == j && self.same_place(x, y),
            (ExprKind::Unary(UnOp::Deref, x), ExprKind::Unary(UnOp::Deref, y)) => self.same_place(x, y),
            (ExprKind::AddrOf(false, ma, x), ExprKind::AddrOf(false, mb, y)) => ma == mb && self.same_place(x, y),
            (ExprKind::Index(x, i), ExprKind::Index(y, j)) => self.same_place(x, y) && self.same_index(i, j),
            _ => false,
        }
    }
    /// The index of a plain `Index` place: a literal (same kind, same token) or a path.
    fn same_index(&self, a: ExprId, b: ExprId) -> bool {
        let ast = self.ast();
        let (ea, eb) = (ast.expr(a), ast.expr(b));
        match (ea.kind, eb.kind) {
            (ExprKind::Lit(ka), ExprKind::Lit(kb)) => ka == kb && self.pf().bytes(ea.lo, ea.hi) == self.pf().bytes(eb.lo, eb.hi),
            (ExprKind::Path(_), ExprKind::Path(_)) => self.same_place(a, b),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "struct V { a: Vec<u32>, b: Vec<u32> }\nfn add(v: &mut V, x: u32) {}\nfn count(v: &mut V) -> u32 { 0 }\nfn draw(v: &mut V, s: &Vec<u32>) {}\nfn clear(v: &mut V) {}\nfn f(vm: &mut V) {\n    add(vm, count(vm));\n}\nfn g(vm: &mut V) {\n    draw(vm, &vm.a);\n}\nfn h(vm: &mut V) -> u32 {\n    let first = &vm.a[0];\n    clear(vm);\n    *first\n}\nfn k(vm: &mut V) {\n    let c = || add(vm, count(vm));\n}\nfn pair(v: &mut V, s: &Vec<u32>, n: usize) {}\nfn p(vm: &mut V) {\n    pair(vm, &vm.a, (vm .a).len());\n}\nfn main() {}\n";

    fn prog() -> Program {
        let session = std::sync::Arc::new(crate::Session::default());
        let path = std::env::temp_dir().join(format!("relax-fvedit-{}-{:?}.rs", std::process::id(), std::thread::current().id()));
        std::fs::write(&path, SRC).unwrap();
        let mut prog = Program::new(&session);
        prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        std::fs::remove_file(path).unwrap();
        prog
    }
    fn sp(prog: &Program, needle: &str, nth: usize) -> Span {
        let file = (prog.files.len() - 1) as u32;
        let lo = SRC.match_indices(needle).nth(nth).unwrap().0 as u32;
        Span { file, lo, hi: lo + needle.len() as u32 }
    }
    /// `inner` within the `nth` occurrence of `outer`.
    fn within(prog: &Program, outer: &str, nth: usize, inner: &str, inner_nth: usize) -> Span {
        let o = sp(prog, outer, nth);
        let lo = o.lo + SRC[o.lo as usize..o.hi as usize].match_indices(inner).nth(inner_nth).unwrap().0 as u32;
        Span { file: o.file, lo, hi: lo + inner.len() as u32 }
    }
    fn rec(span: Span, access: Span, default: bool, hoist: Option<Vec<Span>>) -> FvRecordRaw {
        FvRecordRaw { span, place: None, default, access, hoist }
    }
    fn run(prog: &Program, r: FvRecordRaw) -> (String, Vec<String>, Vec<String>) {
        let mut plans = plan(prog, &[r]);
        let p = plans.pop().unwrap();
        (String::from_utf8(crate::relaxed::apply(SRC.as_bytes(), &p.edits).0).unwrap(), p.applied, p.skipped)
    }

    #[test]
    fn counts_the_place_by_structure_not_by_text() {
        let prog = prog();
        // `(vm .a)` is the same place as `vm.a` though its text differs
        let (_, applied, skipped) = run(&prog, rec(within(&prog, "pair(vm, &vm.a, (vm .a).len())", 0, "&vm.a", 0), within(&prog, "pair(vm, &vm.a, (vm .a).len())", 0, "vm", 0), true, None));
        assert!(applied.is_empty() && skipped[0].contains("more than once"), "{skipped:?}");
    }

    #[test]
    fn hoists_the_argument_holding_the_second_borrow() {
        let prog = prog();
        let stmt = "add(vm, count(vm))";
        let arg = within(&prog, stmt, 0, "count(vm)", 0);
        let (out, applied, skipped) = run(&prog, rec(arg, within(&prog, stmt, 0, "vm", 1), false, Some(vec![within(&prog, stmt, 0, "vm", 0)])));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(out.contains("    let __fv0 = count(vm);\n    add(vm, __fv0);"), "{out}");
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn moves_the_field_out_around_a_statement() {
        let prog = prog();
        // `&vm.a` in `draw(vm, &vm.a);` conflicts with the whole `vm`
        let (out, _, skipped) = run(&prog, rec(sp(&prog, "&vm.a", 0), within(&prog, "draw(vm, &vm.a)", 0, "vm", 0), true, None));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(out.contains("    let mut __fv0 = std::mem::take(&mut vm.a);\n    draw(vm, &__fv0);\n    vm.a = __fv0;"), "{out}");
        // without Default the rewrite is reported, not guessed
        let (_, applied, skipped) = run(&prog, rec(sp(&prog, "&vm.a", 0), within(&prog, "draw(vm, &vm.a)", 0, "vm", 0), false, None));
        assert!(applied.is_empty() && skipped[0].contains("no Default"), "{skipped:?}");
    }

    #[test]
    fn borrows_a_let_again_after_the_conflicting_statement() {
        let prog = prog();
        let (out, _, skipped) = run(&prog, rec(sp(&prog, "&vm.a[0]", 0), sp(&prog, "clear(vm)", 0), false, None));
        assert!(skipped.is_empty(), "{skipped:?}");
        assert!(out.contains("    clear(vm);\n    let first = &vm.a[0];\n    *first"), "{out}");
    }

    const MAPS: &str = "use std::collections::HashMap;\nfn d<'a>(map: &'a mut HashMap<u32, u32>, key: u32) -> &'a mut u32 {\n    match map.get_mut(&key) {\n        Some(v) => v,\n        None => { map.insert(key, 0); map.get_mut(&key).unwrap() }\n    }\n}\nfn e<'a>(map: &'a mut HashMap<u32, u32>, key: u32) -> &'a mut u32 {\n    if let Some(v) = map.get_mut(&key) {\n        return v;\n    }\n    map.insert(key, 0);\n    map.get_mut(&key).unwrap()\n}\nfn k() -> u32 { 0 }\nfn f<'a>(map: &'a mut HashMap<u32, u32>) -> &'a mut u32 {\n    if let Some(v) = map.get_mut(&k()) {\n        return v;\n    }\n    map.get_mut(&1).unwrap()\n}\nfn main() {}\n";

    fn map_prog() -> Program {
        let session = std::sync::Arc::new(crate::Session::default());
        let path = std::env::temp_dir().join(format!("relax-relookup-{}-{:?}.rs", std::process::id(), std::thread::current().id()));
        std::fs::write(&path, MAPS).unwrap();
        let mut prog = Program::new(&session);
        prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        std::fs::remove_file(path).unwrap();
        prog
    }
    /// The relaxed note of a Polonius acceptance whose loan is the lookup `call` (the `nth` occurrence).
    fn polonius_note(prog: &Program, call: &str, nth: usize) -> crate::relaxed::Note {
        let file = (prog.files.len() - 1) as u32;
        let lo = MAPS.match_indices(call).nth(nth).unwrap().0 as u32;
        let span = Span { file, lo, hi: lo + call.len() as u32 };
        let rec = super::super::RelaxedRecord::Polonius(super::super::PoloniusRecord { code: "E0499", span, loan: span,
            lookup: Some(super::super::Lookup { mutable: true, call: span }) });
        plan_relaxed(prog, &[rec]).pop().unwrap()
    }

    #[test]
    fn relookup_rewrites_a_match_and_an_if_let() {
        let prog = map_prog();
        let n = polonius_note(&prog, "map.get_mut(&key)", 0);
        let (_, edits) = n.fix.expect("a match on the lookup is rewritten");
        let out = String::from_utf8(crate::relaxed::apply(MAPS.as_bytes(), &edits).0).unwrap();
        assert!(out.contains("    if map.contains_key(&key) { let v = map.get_mut(&key).unwrap(); v } else { map.insert(key, 0); map.get_mut(&key).unwrap() }\n}"), "{out}");
        let n = polonius_note(&prog, "map.get_mut(&key)", 2);
        let (_, edits) = n.fix.expect("an if-let on the lookup is rewritten");
        let out = String::from_utf8(crate::relaxed::apply(MAPS.as_bytes(), &edits).0).unwrap();
        assert!(out.contains("    if map.contains_key(&key) {\n        let v = map.get_mut(&key).unwrap();\n        return v;\n    }"), "{out}");
        assert!(n.msg.contains("accepted under Polonius"), "{}", n.msg);
    }

    #[test]
    fn relookup_needs_a_side_effect_free_key() {
        let prog = map_prog();
        let n = polonius_note(&prog, "map.get_mut(&k())", 0);
        assert!(n.fix.is_none() && n.msg.contains("no known rewrite") && n.msg.contains("plain place"), "{}", n.msg);
    }

    const PAIRS: &str = "fn a(v: &mut Vec<u32>, i: usize) {\n    let x = &mut v[i];\n    let y = &mut v[i + 1];\n    *x += *y;\n}\nfn b(v: &mut Vec<u32>, i: usize) {\n    let x = &mut v[i];\n    v.len();\n    let y = &mut v[i + 1];\n    *x += *y;\n}\nfn c(v: &mut Vec<u32>, i: usize) {\n    let x = &v[i];\n    let y = &mut v[i + 1];\n    *y += *x;\n}\nfn main() {}\n";

    fn pairs_prog() -> Program {
        let session = std::sync::Arc::new(crate::Session::default());
        let path = std::env::temp_dir().join(format!("relax-pairs-{}-{:?}.rs", std::process::id(), std::thread::current().id()));
        std::fs::write(&path, PAIRS).unwrap();
        let mut prog = Program::new(&session);
        prog.load_crate("custom", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
        std::fs::remove_file(path).unwrap();
        prog
    }
    /// The relaxed note of a D3 acceptance of the index expressions `first`, `second` (the `nth` pair).
    fn disjoint_note(prog: &Program, first: &str, second: &str, nth: usize, both_mut: bool) -> crate::relaxed::Note {
        let file = (prog.files.len() - 1) as u32;
        let span = |t: &str| { let lo = PAIRS.match_indices(t).nth(nth).unwrap().0 as u32; Span { file, lo, hi: lo + t.len() as u32 } };
        let rec = super::super::RelaxedRecord::Disjoint(super::super::DisjointRecord { owner: crate::program::DefId(0), first: span(first), second: span(second),
            proven: true, both_mut, code: "E0499" });
        plan_relaxed(prog, &[rec]).pop().unwrap()
    }

    #[test]
    fn disjoint_pairs_become_get_disjoint_mut() {
        let prog = pairs_prog();
        let n = disjoint_note(&prog, "v[i]", "v[i + 1]", 0, true);
        let (_, edits) = n.fix.expect("two consecutive lets are rewritten");
        let out = String::from_utf8(crate::relaxed::apply(PAIRS.as_bytes(), &edits).0).unwrap();
        assert!(out.contains("fn a(v: &mut Vec<u32>, i: usize) {\n    let [x, y] = v.get_disjoint_mut([i, i + 1]).unwrap();\n    *x += *y;"), "{out}");
        // a statement between the borrows: not rewritten (it would run before both)
        let n = disjoint_note(&prog, "v[i]", "v[i + 1]", 1, true);
        assert!(n.fix.is_none() && n.msg.contains("not consecutive"), "{}", n.msg);
        // a shared borrow: get_disjoint_mut would make it mutable
        let n = disjoint_note(&prog, "v[i]", "v[i + 1]", 2, false);
        assert!(n.fix.is_none() && n.msg.contains("shared"), "{}", n.msg);
    }

    #[test]
    fn conditional_arguments_are_not_hoisted() {
        let prog = prog();
        let stmt = "add(vm, count(vm))";
        let arg = within(&prog, stmt, 1, "count(vm)", 0);
        let (_, applied, skipped) = run(&prog, rec(arg, within(&prog, stmt, 1, "vm", 1), false, Some(vec![within(&prog, stmt, 1, "vm", 0)])));
        assert!(applied.is_empty() && skipped[0].contains("conditionally"), "{skipped:?}");
    }
}
