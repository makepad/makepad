//! Closure environment construction from typeck's authoritative capture result.
//! Virtual source places (VBASE + typeck local) are rewritten through captured
//! place prefixes; no use collection or capture inference runs in borrowck.

use super::bir::*;
use super::build::{Build, ClosureOut, VBASE};
use crate::types::{TyId, TyKind};

/// A capture in the enclosing body's terms.
#[derive(Clone, Debug)]
pub struct CapOut {
    pub root: u32,
    pub proj: Vec<Proj>,
    /// 0 by value, 1 shared, 2 mutable, 3 unique immutable
    pub kind: u8,
    pub ty: TyId,
    /// the use in the closure body that caused the capture
    pub span: Span,
}

/// Convert the typeck capture result to BIR places. Capture inference belongs to
/// typeck (rustc_hir_typeck/upvar.rs); this layer only installs the environment
/// and rewrites source-place prefixes (rustc_mir_build/expr/as_place.rs:172).
pub fn build_closure(parent: &mut Build, ce: u32, ct: TyId) -> Vec<CapOut> {
    let Some(result) = parent.b.min_captures.get(&ce) else {
        parent.out.unsupported = Some("closure capture analysis unavailable".to_string());
        return Vec::new();
    };
    if let Some((_, reason)) = result.errors.first() {
        parent.out.unsupported = Some(format!("incomplete closure capture analysis: {}", reason));
        return Vec::new();
    }
    let inferred_kind = match result.kind {
        crate::upvar::ClosureKind::Fn => ClosureKind::Fn,
        crate::upvar::ClosureKind::FnMut => ClosureKind::FnMut,
        crate::upvar::ClosureKind::FnOnce => ClosureKind::FnOnce,
    };
    // An expected call-trait kind constrains the closure body's receiver, as in
    // rustc's analyze_closure. It does not change which places are captured.
    let kind = parent.expected_closure_kind(ce).unwrap_or(inferred_kind);
    let mut sub = parent.sub_builder(ce);
    sub.build_closure_body(ce, kind, ct);
    // ---- the env and the rewrite
    let mut out = Vec::new();
    let mut cap_decl = Vec::new();
    for capture in &result.captures {
        let root = capture.place.root;
        let root_ty = capture.place.root_ty;
        let pt = capture.place.ty();
        let mut proj = Vec::new();
        for (i, (projection, ty)) in capture.place.projections.iter().enumerate() {
            match projection {
                crate::expr_use::Projection::Deref => proj.push(Proj::Deref),
                crate::expr_use::Projection::Field(variant, field) => {
                    if matches!(sub.tx.kind(capture.place.before(i)), TyKind::Adt(d, _, _) if sub.tx.prog.def(d).kind == crate::program::DefKind::Enum) {
                        proj.push(Proj::Downcast(*variant));
                    }
                    proj.push(Proj::Field(*field, *ty));
                }
                crate::expr_use::Projection::Index | crate::expr_use::Projection::Subslice => {
                    unreachable!("capture precision must exclude index and subslice projections");
                }
            }
        }
        let span = Span { lo: capture.info.path_span.lo, hi: capture.info.path_span.hi };
        let kind_n = match capture.info.kind {
            crate::upvar::CaptureKind::ByValue => 0,
            crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::Imm) => 1,
            crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::Mut) => 2,
            crate::upvar::CaptureKind::ByRef(crate::expr_use::BorrowKind::UniqueImm) => 3,
        };
        let fty = match kind_n {
            0 => pt,
            1 => sub.tx.ref_ty(false, pt),
            _ => sub.tx.ref_ty(true, pt),
        };
        let name = name_of(&sub, root, &proj);
        // rustc determine_capture_mutability: the binding's, made mutable by a deref of `&mut`,
        // immutable after a deref of `&`
        let mut mutable = parent.b.locals[root as usize].mutable || parent.locals.get(&root).map_or(false, |p| p.local < VBASE && parent.out.locals.get(p.local as usize).map_or(false, |l| l.mutable));
        for (i, pr) in proj.iter().enumerate() {
            if *pr == Proj::Deref {
                match sub.tx.kind(ty_before(&sub, root_ty, &proj, i)) {
                    TyKind::Ref(true, _, _) => mutable = true,
                    TyKind::Ref(false, _, _) => {
                        mutable = false;
                        break;
                    }
                    _ => {}
                }
            }
        }
        cap_decl.push(Capture { place: Place { local: VBASE + root, proj: proj.clone() }, ty: fty, kind: kind_n, name, span, mutable });
        out.push(CapOut { root, proj: proj.clone(), kind: kind_n, ty: pt, span });
    }
    rewrite(&mut sub.out, &cap_decl, kind);
    // Copy is also a typeck result; do not ask a second obligation query with a
    // potentially different generic environment while constructing BIR.
    parent.tx.closure_copy.borrow_mut().insert((parent.out.file, ce), result.is_copy);
    parent.tx.reset_copy_memo();
    sub.out.captures = cap_decl;
    sub.out.closure_kind = Some(kind);
    parent.closure_kinds.insert(ce, kind);
    // nested closures built inside this one belong to the list too
    let nested = std::mem::take(&mut sub.closures_out);
    for (k, v) in sub.closure_kinds.iter() {
        parent.closure_kinds.insert(*k, *v);
    }
    let bir = std::mem::replace(&mut sub.out, empty_bir());
    parent.closures_out.push(ClosureOut { expr: ce, bir, kind });
    parent.closures_out.extend(nested);
    out
}

fn empty_bir() -> Bir {
    Bir {
        checked_body: None,
        blocks: Vec::new(),
        locals: Vec::new(),
        n_args: 0,
        regions: Vec::new(),
        known: Vec::new(),
        outlives: Vec::new(),
        captures: Vec::new(),
        closure_kind: None,
        closures: Vec::new(),
        unsupported: None,
        file: 0,
        name: String::new(),
        def: None,
        args: Vec::new(),
        closure_expr: None,
        module: 0,
        user_tys: Vec::new(),
        rvalue_user_tys: Vec::new(),
        local_user_tys: Vec::new(),
        ascriptions: Vec::new(),
            agg_callee: Vec::new(),
            hr_args: Vec::new(),
            ov_results: Vec::new(),
            owner: None,
            capture_spans: Vec::new(),
            guard_locals: Vec::new(),
            index_fakes: Vec::new(),
            expected_sig: false,
            static_temps: Vec::new(),
            static_callees: Vec::new(),
            infer_params: Vec::new(),
            union_inits: Vec::new(),
            const_uses: Vec::new(),
            hr_ret: None,
            move_sites: Vec::new(),
            call_user_tys: Vec::new(),
            fn_values: Vec::new(),
            fn_anns: Vec::new(),
            wf_annots: Vec::new(),
    }
}

fn is_prefix(a: &[Proj], b: &[Proj]) -> bool {
    a.len() <= b.len() && a == &b[..a.len()]
}

fn ty_step(b: &Build, t: TyId, p: &Proj) -> TyId {
    match p {
        Proj::Deref => b.tx.builtin_deref(t).unwrap_or(t),
        Proj::Field(_, ft) => *ft,
        Proj::Index(_) | Proj::ConstIndex(..) => match b.tx.kind(t) {
            TyKind::Array(e, _) | TyKind::Slice(e) => e,
            _ => t,
        },
        Proj::Subslice(..) | Proj::Downcast(_) => t,
    }
}

fn ty_before(b: &Build, root: TyId, proj: &[Proj], i: usize) -> TyId {
    let mut t = root;
    for p in &proj[..i] {
        t = ty_step(b, t, p);
    }
    t
}

fn name_of(b: &Build, root: u32, proj: &[Proj]) -> String {
    let mut s = b.tx.prog.syms.str(b.b.locals[root as usize].name).to_string();
    let root_ty = b.b.locals[root as usize].ty;
    let mut t = root_ty;
    let mut vi = 0u32;
    for p in proj {
        match p {
            Proj::Deref => s = format!("*{}", s),
            Proj::Field(fi, ft) => {
                let n = b.tx.field_name(t, vi, *fi);
                s = if s.starts_with('*') { format!("({}).{}", s, n) } else { format!("{}.{}", s, n) };
                t = *ft;
                vi = 0;
                continue;
            }
            Proj::Downcast(v) => {
                vi = *v;
                continue;
            }
            _ => s = format!("{}[..]", s),
        }
        t = ty_step(b, t, p);
    }
    s
}

/// Virtual places -> env places: (*env).k / env.k (+ deref for by-ref captures) + the rest.
fn rewrite(bir: &mut Bir, caps: &[Capture], kind: ClosureKind) {
    let map = |p: &mut Place| {
        if p.local < VBASE {
            return true;
        }
        let root = p.local;
        let mut best: Option<usize> = None;
        for (i, c) in caps.iter().enumerate() {
            if c.place.local == root && is_prefix(&c.place.proj, &p.proj) {
                if best.map_or(true, |b| caps[b].place.proj.len() < c.place.proj.len()) {
                    best = Some(i);
                }
            }
        }
        let i = match best {
            Some(i) => i,
            None => return false,
        };
        let c = &caps[i];
        let mut np = Place::local(1);
        if kind != ClosureKind::FnOnce {
            np = np.deref();
        }
        np = np.project(Proj::Field(i as u32, c.ty));
        if c.kind != 0 {
            np = np.deref();
        }
        np.proj.extend_from_slice(&p.proj[c.place.proj.len()..]);
        *p = np;
        true
    };
    let mut bad = false;
    for blk in &mut bir.blocks {
        for s in &mut blk.stmts {
            let ok = match &mut s.kind {
                StmtKind::Assign(p, rv) => {
                    let mut ok = map(p);
                    match rv {
                        Rvalue::Use(o) | Rvalue::Cast(_, o, _) | Rvalue::Repeat(o) => ok &= map_op(o, &map),
                        Rvalue::Ref(_, _, pl) | Rvalue::RawPtr(_, pl) | Rvalue::Discriminant(pl) | Rvalue::Len(pl) => ok &= map(pl),
                        Rvalue::Aggregate(_, ops) | Rvalue::Op(_, ops) => {
                            for o in ops {
                                ok &= map_op(o, &map);
                            }
                        }
                    }
                    ok
                }
                StmtKind::FakeRead(_, p) | StmtKind::PlaceMention(p) => {
                    if !map(p) {
                        // not captured (a fake read only): nothing to check
                        s.kind = StmtKind::Nop;
                    }
                    true
                }
                _ => true,
            };
            bad |= !ok;
        }
        if let Some(t) = &mut blk.term {
            let ok = match &mut t.kind {
                TermKind::Switch(o, _) => map_op(o, &map),
                TermKind::Call { args, dest, .. } => {
                    let mut ok = map(dest);
                    for o in args {
                        ok &= map_op(o, &map);
                    }
                    ok
                }
                TermKind::Drop(p, _) => map(p),
                _ => true,
            };
            bad |= !ok;
        }
    }
    // Places the body keeps beside its statements are upvar projections like any other: rustc
    // has them IN the MIR (StatementKind::AscribeUserType, the Call terminator's `func`
    // operand, the place a const pattern is compared with), so they move to the env too.
    // An ascription is a non-use (NonUseContext::AscribeUserTy): a place nothing captured
    // (`let _: T = upvar;`) has no env field and the ascription is dropped, as a FakeRead is.
    bir.ascriptions.retain_mut(|a| map(&mut a.place));
    // a fn item value called through a captured local (`let d = S::new; .. || d(x)`): the
    // callee is captured (it is used by value), so it always maps
    for (_, p) in &mut bir.fn_values {
        bad |= !map(p);
    }
    for c in &mut bir.const_uses {
        if let Some(p) = &mut c.matched {
            if !map(p) {
                c.matched = None;
            }
        }
    }
    if bad && bir.unsupported.is_none() {
        bir.unsupported = Some("closure capture not found".to_string());
    }
}

fn map_op(o: &mut Operand, map: &dyn Fn(&mut Place) -> bool) -> bool {
    match o {
        Operand::Copy(p) | Operand::Move(p) => map(p),
        Operand::Const(_) => true,
    }
}
