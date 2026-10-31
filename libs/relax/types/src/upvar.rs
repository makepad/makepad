//! Capture inference over typed expression-use events.
//! rustc_hir_typeck/src/upvar.rs:629 (processing), :775 (minimum captures),
//! :1998 (packed), :2176 (raw pointers), :2194 (Drop), :2546 (truncation).
use crate::ast::{BlockId, ExprId, ExprKind};
use crate::tcx::EvalCx;

/// The type queries capture analysis asks: the tables and the body's evaluation context.
type TyQ<'a, 'b> = (&'a Tcx, &'a EvalCx<'b>);
use crate::expr_use::{BorrowKind, ConsumeMode, Delegate, ExprUseVisitor, FakeReadCause, Place, Projection, Span};
use crate::program::Program;
use crate::tcx::Tcx;
use crate::typeck::Body;
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ClosureKind { Fn, FnMut, FnOnce }
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CaptureKind { ByRef(BorrowKind), ByValue }
pub(crate) fn is_capture_owner(tcx: &Tcx, body: &Body, file: u32, owner: TyId) -> bool {
    file == body.file && matches!(tcx.tys.kind(owner), TyKind::FnDef(def, args, _) if *def == body.def && tcx.tys.list(*args) == &body.args[..])
}

/// ClosureArgs::kind_ty (rustc_type_ir/src/ty_kind/closure.rs:173-190): the owner's upvar
/// analysis unifies it (upvar.rs:457-465) and writeback resolves it, so another body reads the
/// kind from the closure type itself. Only the current owner, still inside its postorder
/// analysis, reads the in-progress result. None while the kind is not yet known.
pub(crate) fn closure_kind(tcx: &Tcx, body: &Body, file: u32, ce: u32, args: crate::types::ClosureArgs) -> Option<ClosureKind> {
    if is_capture_owner(tcx, body, file, args.parent_args) {
        return body.min_captures.get(&ce).filter(|c| c.errors.is_empty()).map(|c| c.kind);
    }
    tcx.tys.to_opt_closure_kind(args.kind_ty())
}

#[derive(Clone, Debug)]
pub struct CaptureInfo {
    pub kind: CaptureKind,
    pub path_span: Span,
    pub kind_span: Span,
}
#[derive(Clone, Debug)]
pub struct Capture { pub place: Place, pub info: CaptureInfo }
#[derive(Clone, Debug)]
pub struct ClosureCaptures {
    pub captures: Vec<Capture>,
    /// rustc `closure_fake_reads`: (place, cause, the scrutinee / initializer expression)
    pub fake_reads: Vec<(Place, FakeReadCause, u32)>,
    pub kind: ClosureKind,
    pub is_copy: bool,
    pub kind_origin: Option<Span>,
    pub errors: Vec<(u32, &'static str)>,
    pub coroutine: Option<CoroutineClosureCaptures>,
    pub suspension_points: Option<u32>,
}

/// Typeck leaves this upvar tuple variable open until postorder analysis.
pub(crate) enum CaptureSource {
    Expr(ExprId),
    Block(BlockId),
}
pub(crate) struct PendingCapture {
    pub closure: u32,
    pub upvars: TyId,
    pub kind: TyId,
    pub source: CaptureSource,
    pub is_move: bool,
    pub force_args: Vec<u32>,
    pub force_move_if_consumes: bool,
    pub points: Option<u32>,
}

/// The producer's two coroutine environments. Lowering can use the remapping
/// to build rustc's AsyncFnOnce body without reanalyzing expressions.
#[derive(Clone, Debug)]
pub struct CoroutineClosureCaptures {
    pub coroutine: u32,
    pub by_move: Vec<Capture>,
    pub child_captures: Vec<CoroutineCapture>,
}
#[derive(Clone, Debug)]
pub struct CoroutineCapture {
    pub child: usize,
    pub parent: usize,
    pub borrow_from_env: bool,
    pub peel_deref: bool,
    pub projections: Vec<(Projection, TyId)>,
}

struct InferBorrowKind<'a, 'tx> {
    tx: &'a TyQ<'a, 'tx>,
    roots: &'a [u32],
    uses: Vec<Capture>,
    fake_reads: Vec<(Place, FakeReadCause, u32)>,
}
impl InferBorrowKind<'_, '_> {
    fn record(&mut self, place: Place, kind: CaptureKind) {
        if !self.roots.contains(&place.root) { return; }
        let span = place.span;
        self.uses.push(Capture { place, info: CaptureInfo { kind, path_span: span, kind_span: span } });
    }
}
impl Delegate for InferBorrowKind<'_, '_> {
    fn consume(&mut self, place: Place, mode: ConsumeMode) {
        match mode { ConsumeMode::Copy => self.borrow(place, BorrowKind::Imm), ConsumeMode::Move => self.record(place, CaptureKind::ByValue) }
    }
    fn borrow(&mut self, place: Place, kind: BorrowKind) {
        let span = place.span;
        let mut c = Capture { place, info: CaptureInfo { kind: CaptureKind::ByRef(kind), path_span: span, kind_span: span } };
        restrict_packed(self.tx, &mut c);
        if (0..c.place.projections.len()).any(|i| c.place.projections[i].0 == Projection::Deref && matches!(self.tx.0.resolved_kind(self.tx.1, c.place.before(i)), TyKind::Ptr(..))) {
            c.info.kind = CaptureKind::ByRef(BorrowKind::Imm);
        }
        self.record(c.place, c.info.kind);
    }
    fn mutate(&mut self, place: Place) { self.borrow(place, BorrowKind::Mut); }
    fn fake_read(&mut self, place: Place, cause: FakeReadCause, node: u32) {
        if !self.roots.contains(&place.root) { return; }
        let span = place.span;
        let mut c = Capture { place, info: CaptureInfo { kind: CaptureKind::ByRef(BorrowKind::Imm), path_span: span, kind_span: span } };
        restrict_precision(self.tx, &mut c);
        restrict_packed(self.tx, &mut c);
        self.fake_reads.push((c.place, cause, node));
    }
}

/// Cutting through &mut changes a mutable borrow into a unique immutable
/// borrow of the reference; it does not require a mutable reference binding.
fn truncate(tx: &TyQ, c: &mut Capture, len: usize) {
    if c.info.kind == CaptureKind::ByRef(BorrowKind::Mut) && (len..c.place.projections.len()).any(|i| c.place.projections[i].0 == Projection::Deref && matches!(tx.0.resolved_kind(tx.1, c.place.before(i)), TyKind::Ref(true, _, _))) {
        c.info.kind = CaptureKind::ByRef(BorrowKind::UniqueImm);
    }
    c.place.projections.truncate(len);
}
fn restrict_packed(tx: &TyQ, c: &mut Capture) {
    if let Some(i) = (0..c.place.projections.len()).find(|&i| matches!(c.place.projections[i].0, Projection::Field(..)) && tx.0.is_packed_ty(c.place.before(i))) { truncate(tx, c, i); }
}
fn restrict_precision(tx: &TyQ, c: &mut Capture) {
    if let Some(i) = (0..=c.place.projections.len()).find(|&i| matches!(tx.0.resolved_kind(tx.1, c.place.before(i)), TyKind::Ptr(..)) || tx.0.is_union_ty(c.place.before(i))) { truncate(tx, c, i); }
    if let Some(i) = c.place.projections.iter().position(|p| matches!(p.0, Projection::Index | Projection::Subslice)) { truncate(tx, c, i); }
}
fn optimize(tx: &TyQ, c: &mut Capture) {
    // rustc :2643: truncate only if the rightmost dereference is shared.
    if let Some(i) = c.place.projections.iter().rposition(|p| p.0 == Projection::Deref) {
        if matches!(tx.0.resolved_kind(tx.1, c.place.before(i)), TyKind::Ref(false, _, _)) { truncate(tx, c, i + 1); }
    }
}
fn join_info(dst: &mut CaptureInfo, src: &CaptureInfo, preserve_path: bool) {
    if src.kind > dst.kind {
        let path = dst.path_span;
        *dst = src.clone();
        if preserve_path { dst.path_span = path; }
    }
}
fn compute_min_captures(tx: &TyQ, captures: Vec<Capture>) -> Vec<Capture> {
    let mut min: Vec<Capture> = Vec::new();
    for mut c in captures {
        min.retain(|m| {
            if c.place.projections.len() < m.place.projections.len() && c.place.prefix_of(&m.place) {
                let mut m = m.clone();
                truncate(tx, &mut m, c.place.projections.len());
                join_info(&mut c.info, &m.info, true);
                false
            } else { true }
        });
        if let Some(m) = min.iter_mut().find(|m| m.place.prefix_of(&c.place)) {
            let same = m.place.projections.len() == c.place.projections.len();
            truncate(tx, &mut c, m.place.projections.len());
            join_info(&mut m.info, &c.info, !same);
        } else { min.push(c); }
    }
    min
}

/// Analyze innermost closures first, as closure_analyze does; walking a nested
/// closure then consumes its captures instead of walking its syntax again.
pub(crate) fn analyze(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &mut Body, pending: &PendingCapture) {
    // rustc_hir_typeck/src/expr_use_visitor.rs:210: capture queries use the
    // body's inference context; its unresolved types belong to that table.
    let tx: TyQ = (tcx, cx);
    let pf = &prog.files[body.file as usize];
    let ce = pending.closure;
    let is_move = pending.is_move;
    let roots = body.closure_locals.get(&ce).cloned().unwrap_or_default();
    let delegate = InferBorrowKind {
        tx: &tx,
        roots: &roots,
        uses: Vec::new(),
        fake_reads: Vec::new(),
    };
    let mut visitor = ExprUseVisitor {
        tcx,
        cx,
        body,
        delegate,
        errors: Vec::new(),
    };
    match pending.source {
        CaptureSource::Expr(expr) => visitor.consume_expr(expr),
        CaptureSource::Block(block) => visitor.walk_block(block),
    }
    // upvar.rs:229-281: a consuming move async closure cannot return a
    // coroutine borrowing any of its owned captures. Arguments are excluded.
    let force_move = pending.force_move_if_consumes
        && visitor.delegate.uses.iter().any(|c| {
            let mut c = c.clone();
            restrict_precision(&tx, &mut c);
            optimize(&tx, &mut c);
            c.info.kind == CaptureKind::ByValue && !pending.force_args.contains(&c.place.root)
        });
    let span = match pending.source {
        CaptureSource::Expr(e) => {
            let e = pf.ast.expr(e);
            Span { lo: e.lo, hi: e.hi }
        }
        CaptureSource::Block(b) => {
            let b = pf.ast.block(b);
            Span { lo: b.lo, hi: b.hi }
        }
    };
    // Coroutine arguments precede upvars in rustc's environment tuple.
    let uses = std::mem::take(&mut visitor.delegate.uses);
    for &root in &pending.force_args {
        let place = Place {
            root,
            root_ty: body.locals[root as usize].ty,
            projections: Vec::new(),
            expr: ce,
            span,
        };
        visitor.delegate.record(place, CaptureKind::ByValue);
    }
    visitor.delegate.uses.extend(uses);
    let mut kind = ClosureKind::Fn;
    let mut kind_origin = None;
    let mut captures = Vec::new();
    for mut c in std::mem::take(&mut visitor.delegate.uses) {
        restrict_precision(&tx, &mut c);
        optimize(&tx, &mut c);
        let needed = match c.info.kind {
            CaptureKind::ByValue => ClosureKind::FnOnce,
            CaptureKind::ByRef(BorrowKind::Mut | BorrowKind::UniqueImm) => ClosureKind::FnMut,
            _ => ClosureKind::Fn,
        };
        if needed > kind {
            kind = needed;
            kind_origin = Some(c.info.path_span);
        }
        if is_move || force_move || c.info.kind == CaptureKind::ByValue {
            if let Some(i) = c
                .place
                .projections
                .iter()
                .position(|p| p.0 == Projection::Deref)
            {
                truncate(&tx, &mut c, i);
            }
            c.info.kind = CaptureKind::ByValue;
        }
        if c.info.kind == CaptureKind::ByValue && !c.place.projections.is_empty() {
            match visitor.copy_type(c.place.ty()) {
                Some(false) => {
                    if let Some(i) = (0..c.place.projections.len())
                        .find(|&i| tx.0.has_drop_impl(c.place.before(i)))
                    {
                        truncate(&tx, &mut c, i);
                    }
                }
                None => visitor
                    .errors
                    .push((ce, "unresolved Copy obligation for Drop restriction")),
                _ => (),
            }
        }
        if pf.edition < 2021 {
            truncate(&tx, &mut c, 0);
        }
        captures.push(c);
    }
    // Before RFC 2229 every mentioned upvar is captured, even if the only
    // occurrence is a wildcard pattern (upvar.rs:334-357).
    if pf.edition < 2021 {
        for &root in &roots {
            let place = Place {
                root,
                root_ty: body.locals[root as usize].ty,
                projections: Vec::new(),
                expr: ce,
                span,
            };
            let info = CaptureInfo {
                kind: if is_move {
                    CaptureKind::ByValue
                } else {
                    CaptureKind::ByRef(BorrowKind::Imm)
                },
                path_span: span,
                kind_span: span,
            };
            captures.push(Capture { place, info });
        }
    }
    let captures = compute_min_captures(&tx, captures);
    let copy = pending.points.is_none()
        && captures.iter().all(|c| match c.info.kind {
            CaptureKind::ByValue => visitor.copy_type(c.place.ty()) == Some(true),
            CaptureKind::ByRef(BorrowKind::Imm) => true,
            _ => false,
        });
    let result = ClosureCaptures {
        captures,
        fake_reads: visitor.delegate.fake_reads,
        kind,
        is_copy: copy,
        kind_origin,
        errors: visitor.errors,
        coroutine: None,
        suspension_points: pending.points,
    };
    body.min_captures.insert(ce, result);
}

/// upvar.rs:394-483,1970; coroutine/by_move_body.rs:128-192. Parent capture
/// prefixes determine the call-once environment and each child place remapping.
pub(crate) fn coroutine_closure_views(prog: &Program, tcx: &Tcx, body: &mut Body) {
    let ast = &prog.files[body.file as usize].ast;
    for ce in body.closures.clone() {
        if ce & crate::tcx::ASYNC_FN_BODY != 0 {
            continue;
        }
        let ExprKind::Closure(row) = ast.expr(ExprId(ce)).kind else {
            continue;
        };
        let crate::ast::ClosureRow { is_async: true, body: child, .. } = *ast.row(row) else {
            continue;
        };
        let (Some(parent), Some(inner)) =
            (body.min_captures.get(&ce), body.min_captures.get(&child.0))
        else {
            continue;
        };
        let view = coroutine_closure_view(tcx, child.0, parent, inner);
        body.min_captures.get_mut(&ce).unwrap().coroutine = Some(view);
    }
}

fn coroutine_closure_view(
    tcx: &Tcx,
    coroutine: u32,
    parent: &ClosureCaptures,
    inner: &ClosureCaptures,
) -> CoroutineClosureCaptures {
    let mut by_move: Vec<_> = inner
        .captures
        .iter()
        .filter(|c| !parent.captures.iter().any(|p| p.place.root == c.place.root))
        .cloned()
        .collect();
    by_move.extend(parent.captures.iter().cloned());
    let mut child_captures = Vec::new();
    for (child, c) in inner.captures.iter().enumerate() {
        let Some((parent, p)) = parent
            .captures
            .iter()
            .enumerate()
            .find(|(_, p)| p.place.prefix_of(&c.place))
        else {
            continue;
        };
        let shared_deref = c
            .place
            .projections
            .iter()
            .enumerate()
            .skip(p.place.projections.len())
            .any(|(i, pr)| {
                pr.0 == Projection::Deref
                    && matches!(tcx.tys.kind(c.place.before(i)), TyKind::Ref(false, _, _))
            });
        let borrow_from_env = (p.info.kind == CaptureKind::ByValue && !shared_deref)
            || c.info.kind == CaptureKind::ByRef(BorrowKind::Mut);
        let mut projections = c.place.projections[p.place.projections.len()..].to_vec();
        if matches!(p.info.kind, CaptureKind::ByRef(_)) {
            projections.insert(0, (Projection::Deref, p.place.ty()));
        }
        child_captures.push(CoroutineCapture {
            child,
            parent,
            borrow_from_env,
            peel_deref: matches!(c.info.kind, CaptureKind::ByRef(_)),
            projections,
        });
    }
    CoroutineClosureCaptures {
        coroutine,
        by_move,
        child_captures,
    }
}

pub(crate) fn dump_captures(prog: &Program, body: &Body) {
    if std::env::var_os("MAKEPAD_RUN_DUMP_CAPTURES").is_some() {
        dump(prog, body);
    }
}
pub(crate) fn capture_keys(c: &ClosureCaptures) -> Vec<(u32, String, u8)> {
    let mut keys: Vec<_> = c.captures.iter().map(|c| {
        let mut path = String::new();
        for (p, _) in &c.place.projections {
            match p { Projection::Deref => path.push_str("/d"), Projection::Field(v, f) => path.push_str(&format!("/f{}.{}", v, f)), _ => path.push_str("/index") }
        }
        (c.place.root, path, match c.info.kind { CaptureKind::ByValue => 3, CaptureKind::ByRef(k) => k as u8 })
    }).collect();
    keys.sort(); keys
}
fn dump(prog: &Program, b: &Body) {
    for ce in &b.closures {
        let Some(c) = b.min_captures.get(ce) else {
            continue;
        };
        eprintln!("MAKEPAD_RUN_CAPTURES {} file={} closure={} kind={:?} origin={:?} captures={:?} errors={:?} root_slots={:?} keys={:?} coroutine={:?}", prog.def_path(b.def), b.file, ce, c.kind, c.kind_origin, c.captures, c.errors, b.closure_locals.get(ce), capture_keys(c), c.coroutine);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TyId;
    fn capture(t: TyId, ps: Vec<(Projection, TyId)>, kind: CaptureKind, lo: u32) -> Capture {
        let span = Span { lo, hi: lo + 1 };
        Capture { place: Place { root: 0, root_ty: t, projections: ps, expr: lo, span }, info: CaptureInfo { kind, path_span: span, kind_span: span } }
    }
    fn result(captures: Vec<Capture>) -> ClosureCaptures {
        ClosureCaptures {
            captures,
            fake_reads: Vec::new(),
            kind: ClosureKind::Fn,
            is_copy: false,
            kind_origin: None,
            errors: Vec::new(),
            coroutine: None, suspension_points: None,
        }
    }
    #[test]
    fn coroutine_by_move_merges_child_fields_and_preserves_arguments() {
        let tcx = Tcx::new();
        let t = tcx.tys.bool_;
        let pair = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
        let parent = result(vec![capture(pair, vec![], CaptureKind::ByValue, 1)]);
        let mut arg = capture(t, vec![], CaptureKind::ByValue, 2);
        arg.place.root = 1;
        let child = result(vec![
            arg,
            capture(
                pair,
                vec![(Projection::Field(0, 0), t)],
                CaptureKind::ByRef(BorrowKind::Imm),
                3,
            ),
            capture(
                pair,
                vec![(Projection::Field(0, 1), t)],
                CaptureKind::ByRef(BorrowKind::Mut),
                4,
            ),
        ]);
        let view = coroutine_closure_view(&tcx, 10, &parent, &child);
        assert_eq!(view.by_move.len(), 2);
        assert_eq!(view.by_move[0].place.root, 1);
        assert!(view.by_move[1].place.projections.is_empty());
        assert_eq!(
            view.child_captures
                .iter()
                .map(|c| (c.child, c.parent, c.borrow_from_env, c.peel_deref))
                .collect::<Vec<_>>(),
            vec![(1, 0, true, true), (2, 0, true, true)]
        );
        assert_eq!(
            view.child_captures[1].projections,
            vec![(Projection::Field(0, 1), t)]
        );
    }
    #[test]
    fn coroutine_shared_deref_keeps_referent_lifetime() {
        let tcx = Tcx::new();
        let t = tcx.tys.bool_;
        let r = tcx.tys.intern(TyKind::Ref(false, t, crate::types::RegionId::ERASED));
        let parent = result(vec![capture(r, vec![], CaptureKind::ByValue, 1)]);
        let child = result(vec![capture(
            r,
            vec![(Projection::Deref, t)],
            CaptureKind::ByRef(BorrowKind::Imm),
            2,
        )]);
        let view = coroutine_closure_view(&tcx, 10, &parent, &child);
        assert!(!view.child_captures[0].borrow_from_env);
        assert!(view.child_captures[0].peel_deref);
        assert_eq!(view.by_move[0].info.kind, CaptureKind::ByValue);
    }
    #[test]
    fn coroutine_mutable_capture_reborrows_parent_environment() {
        let tcx = Tcx::new();
        let t = tcx.tys.bool_;
        let parent = result(vec![capture(
            t,
            vec![],
            CaptureKind::ByRef(BorrowKind::Mut),
            1,
        )]);
        let child = result(vec![capture(
            t,
            vec![],
            CaptureKind::ByRef(BorrowKind::Mut),
            2,
        )]);
        let view = coroutine_closure_view(&tcx, 10, &parent, &child);
        assert!(view.child_captures[0].borrow_from_env);
        assert_eq!(
            view.child_captures[0].projections,
            vec![(Projection::Deref, t)]
        );
        assert_eq!(
            view.by_move[0].info.kind,
            CaptureKind::ByRef(BorrowKind::Mut)
        );
    }
    #[test]
    fn minimum_capture_truncates_in_both_encounter_orders() {
        let prog = Program::new(&std::sync::Arc::new(crate::Session::default())); let tcx = Tcx::new(); let state = crate::tcx::EvalState::new(); let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state); let tx: TyQ = (&tcx, &cx);
        let t = tcx.tys.bool_; let r = tcx.tys.intern(TyKind::Ref(true, t, crate::types::RegionId::ERASED));
        let ancestor = capture(r, vec![], CaptureKind::ByRef(BorrowKind::Imm), 1);
        let child = capture(r, vec![(Projection::Deref, t)], CaptureKind::ByRef(BorrowKind::Mut), 2);
        for input in [vec![ancestor.clone(), child.clone()], vec![child, ancestor]] {
            let out = compute_min_captures(&tx, input);
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].info.kind, CaptureKind::ByRef(BorrowKind::UniqueImm));
            assert_eq!(out[0].info.path_span.lo, 1);
            assert_eq!(out[0].info.kind_span.lo, 2);
        }
    }
    #[test]
    fn disjoint_fields_and_same_place_kind_origin() {
        let prog = Program::new(&std::sync::Arc::new(crate::Session::default())); let tcx = Tcx::new(); let state = crate::tcx::EvalState::new(); let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state); let tx: TyQ = (&tcx, &cx);
        let t = tcx.tys.bool_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t, t])));
        let a = capture(tuple, vec![(Projection::Field(0, 0), t)], CaptureKind::ByRef(BorrowKind::Imm), 1);
        let b = capture(tuple, vec![(Projection::Field(0, 1), t)], CaptureKind::ByValue, 2);
        let c = capture(tuple, vec![(Projection::Field(0, 0), t)], CaptureKind::ByRef(BorrowKind::Mut), 3);
        let out = compute_min_captures(&tx, vec![a, b, c]);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].info.path_span.lo, 3);
        assert_eq!(out[0].info.kind_span.lo, 3);
        assert_eq!(out[1].info.kind, CaptureKind::ByValue);
    }
    #[test]
    fn optimization_only_truncates_after_last_deref_when_shared() {
        let prog = Program::new(&std::sync::Arc::new(crate::Session::default())); let tcx = Tcx::new(); let state = crate::tcx::EvalState::new(); let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state); let tx: TyQ = (&tcx, &cx);
        let t = tcx.tys.bool_; let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[t])));
        let m = tcx.tys.intern(TyKind::Ref(true, tuple, crate::types::RegionId::ERASED)); let r = tcx.tys.intern(TyKind::Ref(false, m, crate::types::RegionId::ERASED));
        let mut c = capture(r, vec![(Projection::Deref, m), (Projection::Deref, tuple), (Projection::Field(0, 0), t)], CaptureKind::ByRef(BorrowKind::Imm), 1);
        optimize(&tx, &mut c);
        assert_eq!(c.place.projections.len(), 3);
        let r = tcx.tys.intern(TyKind::Ref(false, tuple, crate::types::RegionId::ERASED));
        let mut c = capture(r, vec![(Projection::Deref, tuple), (Projection::Field(0, 0), t)], CaptureKind::ByRef(BorrowKind::Imm), 1);
        optimize(&tx, &mut c);
        assert_eq!(c.place.projections.len(), 1);
    }
    #[test]
    fn opaque_index_restricts_to_array() {
        let prog = Program::new(&std::sync::Arc::new(crate::Session::default())); let tcx = Tcx::new(); let state = crate::tcx::EvalState::new(); let cx = crate::tcx::EvalCx::new(&prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state); let tx: TyQ = (&tcx, &cx);
        let t = tcx.tys.bool_; let array = tcx.tys.intern(TyKind::Array(t, tcx.tys.const_usize(3)));
        let mut c = capture(array, vec![(Projection::Index, t)], CaptureKind::ByValue, 1);
        restrict_precision(&tx, &mut c);
        assert!(c.place.projections.is_empty());
    }
}
