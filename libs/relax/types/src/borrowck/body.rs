//! Borrow-checking owners. Item roots are scheduled by every driver; typeck children
//! are checked with their root, even when the parent's CFG cannot reach their use.
//! Mirrors rustc_interface::passes::analysis and rustc_mir_build::builder.
use crate::ast::{ExprId, ExprKind, ItemKind};
use crate::program::{DefId, DefKind, Program};
use crate::tcx::Tcx;
use crate::typeck::Body;
use crate::types::TyId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    Function,
    Const,
    Static,
    AssocConst,
    InlineConst(ExprId),
    AnonConst(ExprId),
    Closure(ExprId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckedBody {
    /// Item typeck root, including the generic environment inherited by children.
    pub owner: DefId,
    pub kind: BodyKind,
}

impl CheckedBody {
    pub fn item(prog: &Program, owner: DefId) -> Option<Self> {
        let d = prog.def(owner);
        if !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::Static | DefKind::AssocConst) {
            return None;
        }
        let ast = &prog.files[d.file as usize].ast;
        let kind = match (&d.kind, &ast.item(d.item).kind) {
            (DefKind::Fn | DefKind::AssocFn, ItemKind::Fn(s, b, _))
                if b.is_some() || ast.fn_sig(*s).lazy_body.is_some() => BodyKind::Function,
            (DefKind::Const, ItemKind::Const(_, init)) if init.is_some() => BodyKind::Const,
            (DefKind::AssocConst, ItemKind::Const(_, init)) if init.is_some() => BodyKind::AssocConst,
            (DefKind::Static, ItemKind::Static(_, _, init)) if init.is_some() => BodyKind::Static,
            _ => return None,
        };
        Some(Self { owner, kind })
    }

    pub fn identity_args(self, prog: &Program, tcx: &Tcx) -> Vec<TyId> {
        tcx.generic_indices_of(prog, self.owner).identity_types_consts(&tcx.tys)
    }
}

/// The single item-owner enumeration used by synchronous and background drivers.
pub fn owners(prog: &Program) -> impl Iterator<Item = CheckedBody> + '_ {
    (0..prog.defs.len()).filter_map(|i| CheckedBody::item(prog, DefId(i as u32)))
}

/// typeck-audit 1.27: anonymous constants outside every body -- array lengths and const
/// arguments in signatures and types, enum discriminants -- are typeck roots of their own
/// (rustc_interface passes.rs analysis type-checks every body owner). Those inside a body
/// (repeat counts) are checked with that body.
pub fn signature_anon_consts(prog: &Program, tcx: &Tcx, krate: u32) -> Vec<DefId> {
    anon_consts(prog, tcx, krate, false)
}

/// The anonymous constants written inside a body (const arguments of its paths, repeat counts),
/// with the item owner whose body holds them: nested bodies of their own in rustc, whose late
/// lints run on them (rustc_lint late.rs visit_nested_body). The owner's typeck has fed their types.
pub fn body_anon_consts(prog: &Program, tcx: &Tcx, krate: u32) -> Vec<(DefId, DefId, ExprId)> {
    use crate::def_collector::NodeId;
    anon_consts(prog, tcx, krate, true).into_iter().filter_map(|def| {
        let (_, NodeId::AnonConst(e)) = *prog.def_id_to_node_id.get(&def)? else { return None };
        Some((def, root_owner(prog, def), e))
    }).collect()
}

pub(crate) fn root_owner(prog: &Program, def: DefId) -> DefId {
    let mut owner = prog.def(def).parent;
    while matches!(prog.def(owner).kind, DefKind::Closure | DefKind::AnonConst | DefKind::OpaqueTy) { owner = prog.def(owner).parent; }
    owner
}

fn anon_consts(prog: &Program, tcx: &Tcx, krate: u32, inside: bool) -> Vec<DefId> {
    use crate::def_collector::NodeId;
    let mut out = Vec::new();
    for (&def, &(file, node)) in &prog.def_id_to_node_id {
        let NodeId::AnonConst(e) = node else { continue };
        let d = prog.def(def);
        if d.krate != krate || d.kind != DefKind::AnonConst { continue; }
        // rustc_ast_lowering/src/lib.rs:3005-3022: a trivial parameter path
        // lowers directly to Param, so it has no anonymous body to type-check.
        if tcx.const_ty(prog, def).is_none() { continue; }
        let lo = prog.files[file as usize].ast.expr(e).lo;
        if in_body(prog, root_owner(prog, def), file, lo) == inside { out.push(def); }
    }
    out.sort_unstable_by_key(|d| d.0);
    out
}

fn in_body(prog: &Program, owner: DefId, file: u32, lo: u32) -> bool {
    let d = prog.def(owner);
    if d.file != file || !matches!(d.kind, DefKind::Fn | DefKind::AssocFn | DefKind::Const | DefKind::AssocConst | DefKind::Static) { return false; }
    let pf = &prog.files[file as usize];
    let ast = &pf.ast;
    match ast.item(d.item).kind {
        ItemKind::Fn(_, Some(b), _) => { let b = ast.block(b); pf.expands_within(lo, b.lo, b.hi) }
        ItemKind::Fn(s, None, _) => match ast.fn_sig(s).lazy_body {
            Some(r) if r.hi > r.lo => { let src = prog.src(file); pf.expands_within(lo, src.tok(r.lo as usize).lo, src.tok(r.hi as usize - 1).hi) }
            _ => false,
        },
        ItemKind::Const(_, init) | ItemKind::Static(_, _, init) if init.is_some() => { let x = ast.expr(init.get().unwrap()); pf.expands_within(lo, x.lo, x.hi) }
        _ => false,
    }
}

/// The inline constants of a typeck root (closures' included): nested bodies that are borrow-checked with
/// the root (rustc nested_bodies_within).
pub fn inline_const_children(prog: &Program, tcx: &Tcx, body: &Body) -> Vec<u32> {
    let ast = &prog.files[body.file as usize].ast;
    let mut out = Vec::new();
    for i in 0..body.expr_ty.len() {
        if body.expr_ty[i] == tcx.tys.error { continue; }
        let e = ExprId(if i < body.expr_n1 as usize { body.expr_lo + i as u32 } else { body.expr_lo2 + i as u32 - body.expr_n1 });
        if matches!(ast.expr(e).kind, ExprKind::ConstBlock(_)) && body.inline_const_types.get(&e.0).is_some() { out.push(e.0); }
    }
    out
}

/// Anonymous const children for which typeck retained expression facts (inline consts are
/// nested bodies of the root instead: `inline_const_children`).
pub fn const_children(prog: &Program, tcx: &Tcx, body: &Body) -> Vec<CheckedBody> {
    let ast = &prog.files[body.file as usize].ast;
    let mut out = Vec::new();
    for i in 0..body.expr_ty.len() {
        // Nested item ranges can share the arena; their facts belong to their
        // own typeck root, not this body.
        if body.expr_ty[i] == tcx.tys.error { continue; }
        let e = ExprId(if i < body.expr_n1 as usize {
            body.expr_lo + i as u32
        } else {
            body.expr_lo2 + i as u32 - body.expr_n1
        });
        let kind = match ast.expr(e).kind {
            ExprKind::Repeat(_, n) if !matches!(ast.expr(n).kind, ExprKind::ConstBlock(_))
                && !crate::tcx::is_infer_len(ast, n) => BodyKind::AnonConst(n),
            _ => continue,
        };
        let child = CheckedBody { owner: body.def, kind };
        out.push(child);
    }
    out
}
