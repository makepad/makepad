//! Pattern spans. The AST `Pat` carries no span of its own (only identifiers and literal
//! expressions do), so a pattern's span is derived: the union of its leaves (identifiers, paths,
//! literal expressions), extended over the keywords (`ref`, `mut`) and delimiters (`(`, `[`, `)`,
//! `]`, `{`, `}`) the source shows around them. A pattern with no leaf at all (`_`, `..`) has a
//! zero-width span at the nearest known position (`Cx::pat_hint`); diagnostics use lines, so that
//! is enough.
use crate::ast::{Ast, Pat, PatId};
use crate::mir::Span;
use crate::mir::build::thir::Cx;

impl Cx<'_> {
    /// The derived span of `p` (see the module doc).
    pub fn pat_span(&self, p: PatId) -> Span {
        let file = self.root.typeck.file;
        match self.pat_range(p) {
            Some((lo, hi)) => Span { file, lo, hi },
            None => Span { file, lo: self.pat_hint, hi: self.pat_hint },
        }
    }

    fn src_byte(&self, at: u32) -> Option<u8> {
        let pf = &self.root.cx.prog.files[self.root.typeck.file as usize];
        pf.bytes(at, at.checked_add(1)?).first().copied()
    }

    /// First non-whitespace byte at or after `at`.
    fn skip_ws_forward(&self, mut at: u32) -> Option<(u32, u8)> {
        for _ in 0..4096 {
            let b = self.src_byte(at)?;
            if !b.is_ascii_whitespace() {
                return Some((at, b));
            }
            at += 1;
        }
        None
    }

    /// Last non-whitespace byte strictly before `at`.
    fn skip_ws_backward(&self, mut at: u32) -> Option<(u32, u8)> {
        for _ in 0..4096 {
            if at == 0 {
                return None;
            }
            at -= 1;
            let b = self.src_byte(at)?;
            if !b.is_ascii_whitespace() {
                return Some((at, b));
            }
        }
        None
    }

    /// Extend `(lo, hi)` over one pair of delimiters directly around it (the optional trailing
    /// comma of a list included).
    fn extend_delims(&self, lo: u32, hi: u32, open: u8, close: u8) -> (u32, u32) {
        let mut hi2 = hi;
        let mut cur = self.skip_ws_forward(hi);
        if let Some((at, b',')) = cur {
            cur = self.skip_ws_forward(at + 1);
        }
        let mut lo2 = lo;
        if let Some((at, b)) = cur {
            if b == close {
                hi2 = at + 1;
                if let Some((o, ob)) = self.skip_ws_backward(lo) {
                    if ob == open {
                        lo2 = o;
                    }
                }
            }
        }
        (lo2, hi2)
    }

    /// The identifier-like keywords (`ref`, `mut`) directly before `lo`.
    fn extend_keywords(&self, mut lo: u32) -> u32 {
        for _ in 0..2 {
            let Some((end, last)) = self.skip_ws_backward(lo) else { return lo };
            if !last.is_ascii_alphabetic() {
                return lo;
            }
            let mut start = end;
            while start > 0 && self.src_byte(start - 1).is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                start -= 1;
            }
            let word_ok = (start..=end).filter_map(|i| self.src_byte(i)).collect::<Vec<_>>();
            if word_ok == b"ref" || word_ok == b"mut" || word_ok == b"box" {
                lo = start;
            } else {
                return lo;
            }
        }
        lo
    }

    fn merge(a: Option<(u32, u32)>, b: Option<(u32, u32)>) -> Option<(u32, u32)> {
        match (a, b) {
            (Some((l1, h1)), Some((l2, h2))) => Some((l1.min(l2), h1.max(h2))),
            (a, None) => a,
            (None, b) => b,
        }
    }

    fn pats_range(&self, ast: &Ast, list: crate::ast::List<PatId>) -> Option<(u32, u32)> {
        ast.list(list).iter().fold(None, |acc, x| Self::merge(acc, self.pat_range(*x)))
    }

    /// The source range of `p`, None for a pattern with no leaf (`_`, `..`).
    pub(crate) fn pat_range(&self, p: PatId) -> Option<(u32, u32)> {
        let ast = &self.root.cx.prog.files[self.root.typeck.file as usize].ast;
        match *ast.pat(p) {
            Pat::Wild | Pat::Rest => None,
            Pat::Ident { name, sub, .. } => {
                // the span of a binding ends at its identifier (`x @ P` is spanned by `x`)
                let lo = self.extend_keywords(name.lo);
                let _ = sub;
                Some((lo, name.hi))
            }
            Pat::Lit(e) => {
                let ex = ast.expr(e);
                Some((ex.lo, ex.hi))
            }
            Pat::Range(a, b, _) => {
                let r = [a, b].into_iter().flatten().map(|e| (ast.expr(e).lo, ast.expr(e).hi)).fold(None, |acc, r| Self::merge(acc, Some(r)));
                // `..=hi` / `lo..` : the operator is part of the pattern
                r.map(|(lo, hi)| {
                    let hi = if b.is_none() { self.skip_ws_forward(hi).map_or(hi, |(at, c)| if c == b'.' { self.range_op_end(at) } else { hi }) } else { hi };
                    (lo, hi)
                })
            }
            Pat::Path(path) => {
                let hi = ast.segs(path.segs).last().map(|s| s.name.hi)?;
                Some((path.lo, hi))
            }
            Pat::TupleStruct(path, v) => {
                let hi0 = ast.segs(path.segs).last().map(|s| s.name.hi).unwrap_or(path.lo);
                let inner = self.pats_range(ast, v).map_or(hi0, |(_, h)| h.max(hi0));
                // the closing paren (an all-wild list has no leaf: scan from the path end)
                let hi = self.close_after(hi0, inner, b'(', b')');
                Some((path.lo, hi))
            }
            Pat::Struct(path, fps, _) => {
                let hi0 = ast.segs(path.segs).last().map(|s| s.name.hi).unwrap_or(path.lo);
                let inner = ast.list(fps).iter().fold(None, |acc, f| Self::merge(acc, self.pat_range(f.pat).or(Some((f.name.lo, f.name.hi))))).map_or(hi0, |(_, h)| h.max(hi0));
                let hi = self.close_after(hi0, inner, b'{', b'}');
                Some((path.lo, hi))
            }
            Pat::Tuple(v) => self.group_range(ast, v, b'(', b')'),
            Pat::Slice(v) => self.group_range(ast, v, b'[', b']'),
            Pat::Ref(_, x) => {
                let (lo, hi) = self.pat_range(x)?;
                let lo = self.extend_keywords(lo);
                // the `&` / `&mut` before it
                let lo = match self.skip_ws_backward(lo) {
                    Some((at, b'&')) => at,
                    _ => lo,
                };
                Some((lo, hi))
            }
            Pat::Or(v) => self.pats_range(ast, v),
            Pat::Paren(x) => {
                let (lo, hi) = self.pat_range(x)?;
                Some(self.extend_delims(lo, hi, b'(', b')'))
            }
            Pat::Box(x) => {
                let (lo, hi) = self.pat_range(x)?;
                Some((self.extend_keywords(lo), hi))
            }
            Pat::Mac(m) => {
                let path = ast.row(m).path;
                let hi = ast.segs(path.segs).last().map(|s| s.name.hi)?;
                Some((path.lo, hi))
            }
        }
    }

    fn range_op_end(&self, at: u32) -> u32 {
        let mut end = at;
        while self.src_byte(end).is_some_and(|b| b == b'.' || b == b'=') {
            end += 1;
        }
        end
    }

    /// The end of the group opened after `open_from`: the closing delimiter after the last known
    /// leaf (or after the opening one when the list has none).
    fn close_after(&self, open_from: u32, last_leaf_hi: u32, _open: u8, close: u8) -> u32 {
        let mut at = last_leaf_hi.max(open_from);
        // skip `, ..` and whitespace until the closing delimiter
        for _ in 0..256 {
            match self.skip_ws_forward(at) {
                Some((i, b)) if b == close => return i + 1,
                Some((i, b)) if b == b',' || b == b'.' || b == b'_' => at = i + 1,
                _ => return last_leaf_hi.max(open_from),
            }
        }
        last_leaf_hi.max(open_from)
    }

    fn group_range(&self, ast: &Ast, v: crate::ast::List<PatId>, open: u8, close: u8) -> Option<(u32, u32)> {
        let (lo, hi) = self.pats_range(ast, v)?;
        let (mut lo2, _) = self.extend_delims(lo, hi, open, close);
        // the opening delimiter is found even when the list starts with `_`/`..`
        if lo2 == lo {
            if let Some((o, b)) = self.skip_ws_backward(lo) {
                if b == open {
                    lo2 = o;
                }
            }
        }
        let hi2 = self.close_after(hi, hi, open, close);
        Some((lo2, hi2))
    }
}
