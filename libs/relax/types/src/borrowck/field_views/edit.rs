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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedEdit {
    pub lo: u32,
    pub hi: u32,
    pub text: String,
}

#[derive(Debug)]
pub struct PlannedRewrite {
    pub file: u32,
    pub edits: Vec<TypedEdit>,
    pub applied: Vec<String>,
    pub skipped: Vec<String>,
}

/// Splice `edits` (non-overlapping, any order) into `src`.
pub fn apply(src: &str, edits: &[TypedEdit]) -> String {
    let mut edits: Vec<&TypedEdit> = edits.iter().collect();
    edits.sort_by_key(|e| (e.lo, e.hi));
    let mut out = String::with_capacity(src.len() + 64);
    let mut at = 0usize;
    for e in edits {
        out.push_str(&src[at..e.lo as usize]);
        out.push_str(&e.text);
        at = e.hi as usize;
    }
    out.push_str(&src[at..]);
    out
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
        for i in 0..ast.blocks.len() {
            let id = BlockId(ast.base[3] + i as u32);
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

    fn exprs(&self) -> impl Iterator<Item = (ExprId, crate::ast::Expr)> + '_ {
        let ast = self.ast();
        (0..ast.exprs.len()).map(move |i| { let id = ExprId(ast.base[0] + i as u32); (id, *ast.expr(id)) })
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

fn rewrite_one(v: &FileView<'_>, r: &FvRecordRaw, n: usize) -> Result<(Vec<TypedEdit>, String), &'static str> {
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
            TypedEdit { lo: stmt.lo, hi: stmt.lo, text: format!("let {tmp} = {text};\n{indent}") },
            TypedEdit { lo: arg.lo, hi: arg.hi, text: tmp },
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
                return Ok((vec![TypedEdit { lo: access.hi, hi: access.hi, text: format!("\n{indent}let {bound} = {init_text};") }],
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
        TypedEdit { lo: stmt.lo, hi: stmt.lo, text: format!("let mut {tmp} = std::mem::take(&mut {place_text});\n{indent}") },
        TypedEdit { lo: place.lo, hi: place.hi, text: tmp.clone() },
        TypedEdit { lo: stmt.hi, hi: stmt.hi, text: format!("\n{indent}{place_text} = {tmp};") },
    ], format!("`{place_text}` moved out around the statement (mem::take)")))
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
        (apply(SRC, &p.edits), p.applied, p.skipped)
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

    #[test]
    fn conditional_arguments_are_not_hoisted() {
        let prog = prog();
        let stmt = "add(vm, count(vm))";
        let arg = within(&prog, stmt, 1, "count(vm)", 0);
        let (_, applied, skipped) = run(&prog, rec(arg, within(&prog, stmt, 1, "vm", 1), false, Some(vec![within(&prog, stmt, 1, "vm", 0)])));
        assert!(applied.is_empty() && skipped[0].contains("conditionally"), "{skipped:?}");
    }
}
