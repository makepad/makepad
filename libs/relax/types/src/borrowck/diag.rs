//! Diagnostics in rustc's shape: `error[E0502]: message`, `--> file:line:col`, the source
//! lines with `^^^` (primary) / `---` (secondary) labels, notes and help.

use super::bir::*;
use super::tyx::Tx;
use crate::types::TyKind;

#[derive(Clone, Debug)]
pub struct Diag {
    pub code: &'static str,
    pub msg: String,
    pub span: Span,
    /// (span, text, primary)
    pub labels: Vec<(Span, String, bool)>,
    pub notes: Vec<String>,
    pub helps: Vec<String>,
    /// accepted only because of field views (strict reports it as an error with a note)
    pub fv_only: bool,
}

impl Diag {
    pub fn new(code: &'static str, msg: String, span: Span) -> Diag {
        Diag { code, msg, span, labels: Vec::new(), notes: Vec::new(), helps: Vec::new(), fv_only: false }
    }
    pub fn label(&mut self, s: Span, t: String, primary: bool) {
        self.labels.push((s, t, primary));
    }
    pub fn note(&mut self, t: String) {
        self.notes.push(t);
    }
    pub fn help(&mut self, t: String) {
        self.helps.push(t);
    }
}

/// `x`, `x.f`, `*x`, `self.items`, `v[..]` -- None for temporaries.
pub fn describe_place(tx: &Tx, bir: &Bir, p: &Place) -> Option<String> {
    // closure upvars: (*env).k / env.k / *(*env).k
    let (mut s, start, mut t) = if bir.closure_kind.is_some() && p.local == 1 {
        let mut i = 0;
        if p.proj.first() == Some(&Proj::Deref) {
            i = 1;
        }
        match p.proj.get(i) {
            Some(Proj::Field(k, ft)) => {
                let c = bir.captures.get(*k as usize)?;
                let mut j = i + 1;
                let mut ty = *ft;
                if c.kind != 0 && p.proj.get(j) == Some(&Proj::Deref) {
                    j += 1;
                    ty = tx.builtin_deref(ty).unwrap_or(ty);
                }
                (c.name.clone(), j, ty)
            }
            _ => return None,
        }
    } else {
        let l = &bir.locals[p.local as usize];
        let name = l.name.clone()?;
        (name, 0, l.ty)
    };
    let mut vi = 0u32;
    let mut i = start;
    while i < p.proj.len() {
        let pr = &p.proj[i];
        match pr {
            Proj::Deref => {
                // `(*x).f` reads as `x.f`
                let next_is_field = matches!(p.proj.get(i + 1), Some(Proj::Field(..)) | Some(Proj::Downcast(_)));
                if !next_is_field {
                    s = format!("*{}", s);
                }
                t = tx.builtin_deref(t).unwrap_or(t);
            }
            Proj::Field(fi, ft) => {
                let n = match tx.kind(t) {
                    TyKind::Adt(..) => tx.field_name(t, vi, *fi),
                    _ => fi.to_string(),
                };
                // tuple-variant / tuple-struct fields print as their index
                let n = if n.chars().next().map_or(true, |c| !(c.is_alphanumeric() || c == '_')) { fi.to_string() } else { n };
                s = format!("{}.{}", s, n);
                t = *ft;
                vi = 0;
            }
            Proj::Downcast(v) => {
                // rustc describe_place leaves the downcast out (`x.0`)
                vi = *v;
            }
            Proj::Index(_) | Proj::ConstIndex(..) => {
                s = format!("{}[..]", s);
                t = match tx.kind(t) {
                    TyKind::Array(e, _) | TyKind::Slice(e) => e,
                    _ => t,
                };
            }
            Proj::Subslice(..) => s = format!("{}[..]", s),
        }
        i += 1;
    }
    Some(s)
}

/// Capture source data at the checker boundary; consumers render without a Program.
pub fn to_error(prog: &crate::program::Program, file: u32, owner: crate::program::DefId, d: &Diag) -> super::event::BorrowError {
    use super::event::{BorrowError, Label};
    let pf = &prog.files[file as usize];
    let lc = |pos| { let (l, c) = pf.line_col(pos); (l as u32, c as u32) };
    let src = &pf.src[..pf.src_len as usize];
    let line_text = |l: u32| -> String {
        let mut cur = 1u32;
        let mut start = 0usize;
        for (i, &ch) in src.iter().enumerate() {
            if cur == l { start = i; break; }
            if ch == b'\n' {
                cur += 1;
                if cur == l { start = i + 1; break; }
            }
        }
        if cur != l { return String::new(); }
        let end = src[start..].iter().position(|&c| c == b'\n').map_or(src.len(), |e| start + e);
        String::from_utf8_lossy(&src[start..end]).into_owned()
    };
    let (line, col) = lc(d.span.lo);
    let (end_line, end_col) = lc(d.span.hi);
    BorrowError {
        fn_path: prog.def_path(owner), owner, krate: prog.def(owner).krate,
        code: d.code, message: d.msg.clone(), file: prog.file_paths[file as usize].clone(),
        line, col, end_line, end_col,
        labels: d.labels.iter().map(|(s, text, primary)| {
            let (line, col) = lc(s.lo);
            let (end_line, end_col) = lc(s.hi.max(s.lo + 1));
            Label { line, col, end_line, end_col, text: text.clone(), primary: *primary, source: line_text(line) }
        }).collect(),
        notes: d.notes.clone(), helps: d.helps.clone(), fv_only: d.fv_only,
    }
}
