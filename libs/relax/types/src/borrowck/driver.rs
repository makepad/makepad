//! The borrow-check driver (S11): one typeck root through the rustc pipeline on MIR.
//!
//!   owners (body.rs) -> mir::build (S3/S4) -> transform::mir_promoted (S5: const check, promotion)
//!   -> MoveData + signature regions + N01 inference context -> type_check_body (S7)
//!   -> root_cx::do_mir_borrowck (nested bodies first, opaque schedule) -> diagnostics (S9)
//!   + field views (S15) -> `Verdict` (errors, unchecked bodies with reasons, body count).
//!
//! `check_root` is the one function every caller reaches (`check_body`, the lazy worker, `check_all`,
//! `check_before_run`, the hot-patch gate, `makepad check`). A body the builder or a query cannot
//! handle is *unchecked with a reason* (counted, never clean, never an error). A root whose nested body
//! is unchecked is unchecked as a whole: the nested body's requirements cannot be applied.

use super::body::{self, BodyKind as OwnerKind, CheckedBody};
use super::borrow_set::BorrowSet;
use super::diagnostics::region_name::{ExternalName, InputInfo, OutputInfo, UniversalNames};
use super::diagnostics::{self, DiagCx};
use super::field_views::{self, FieldViews, FvBodies, FvBody, SummaryCx};
use super::infer_ctxt::BorrowckInferCtxt;
use super::mir_nll::{type_check_body, MirNllInput, MirTypeck};
use super::root_cx::{BorrowCheckRootCtxt, RootBody, RootBodyOps};
use super::type_check::ClauseOwner;
use super::universal_regions::{Identity, SignatureRegions};
use super::{event, Verdict};
use crate::mir::build::{build_mir, BuildCx, BuildRoot};
use crate::mir::move_paths::MoveData;
use crate::mir::build::driver::{closure_children, BuildProvider};
use crate::mir::transform::{mir_promoted, qualifs::Blocked};
use crate::mir::{Body, BodyKind, ClosureId, Span};
// has_loans: rustc passes `borrow_set.len() != 0`, built from the renumbered body; type_check_body renumbers
// inside, so the driver passes `true` (always compute liveness for loans) -- a perf difference, not a verdict one.
use crate::program::{DefId, Program};
use crate::tcx::{EvalCx, Tcx, TypingEnv};
use crate::typeck;
use crate::types::TyId;
use std::rc::Rc;

/// What a body of the root is, before it is checked.
struct Prepared<'a> {
    owner: CheckedBody,
    key: ClauseOwner,
    nested: bool,
    mir: Box<Body>,
    moves: MoveData,
    sig: SignatureRegions,
    infcx: BorrowckInferCtxt<'a>,
    typeck: Option<MirTypeck>,
}

/// The closure-like children of a typeck root (closures, async blocks), inner before outer.
fn closures_inner_first(prog: &Program, tcx: &Tcx, body: &typeck::Body) -> Vec<u32> {
    let ast = &prog.files[body.file as usize].ast;
    let mut closures: Vec<u32> = closure_children(prog, tcx, body).into_iter().filter_map(|c| match c.kind { OwnerKind::Closure(e) => Some(e.0), _ => None }).collect();
    closures.sort_by_key(|&c| { let (lo, hi) = closure_extent(ast, c); (hi - lo, c) });
    closures
}

/// The source extent of a closure-like child: a closure or async block's expression, or the body
/// block of an async fn (`ASYNC_FN_BODY | block`).
fn closure_extent(ast: &crate::ast::Ast, c: u32) -> (u32, u32) {
    if c & crate::tcx::ASYNC_FN_BODY != 0 {
        let b = ast.block(crate::ast::BlockId(c & !crate::tcx::ASYNC_FN_BODY));
        (b.lo, b.hi)
    } else {
        let e = ast.expr(crate::ast::ExprId(c));
        // (a macro-expanded body can carry a span that ends before it starts: it ends where it ends)
        if e.lo > e.hi { (e.hi, e.hi) } else { (e.lo, e.hi) }
    }
}

/// The closures a body (and its promoted bodies) creates.
fn created_closures(mir: &Body, out: &mut Vec<u32>) {
    use crate::mir::{AggregateKind, Rvalue, StatementKind};
    for block in &mir.basic_blocks {
        for s in &block.statements {
            if let StatementKind::Assign(_, Rvalue::Aggregate(k, _)) = &s.kind {
                if let AggregateKind::Closure(id, _) | AggregateKind::Coroutine(id, _) | AggregateKind::CoroutineClosure(id, _) = &**k { out.push(id.expr); }
            }
        }
    }
    for p in &mir.promoted { created_closures(p, out); }
}

/// Check one typeck root. See the module doc.
pub fn check_root(cx: &EvalCx<'_>, prog: &Program, tcx: &Tcx, typeck: &typeck::Body, fv_cache: Option<&Rc<SummaryCx>>) -> Verdict {
    let mut v = Verdict { errors: Vec::new(), unchecked: Vec::new(), fv_only: Vec::new(), fv_records: Vec::new(), relaxed_records: Vec::new(), bodies: 0 };
    let name = prog.def_path(typeck.def);
    let env = TypingEnv {
        limit: crate::tcx::RecursionLimit::for_item(prog, typeck.def),
        param_env: typeck.param_env.unwrap_or_default(),
        mode: crate::tcx::TypingMode::borrowck(tcx, prog, typeck.def),
    };
    let eval = EvalCx::new(prog, tcx, env, cx.state);
    let root = match BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, typeck) {
        Ok(r) => r,
        Err(why) => { v.unchecked.push(format!("{name}: {why}")); return v; }
    };

    // The item body with its closures (inner first), then each array-length constant as an
    // independent root of its own (rustc: an anon const is its own typeck root and `mir_borrowck`).
    let mut owners: Vec<CheckedBody> = Vec::new();
    let closure_order = closures_inner_first(prog, tcx, typeck);
    for &c in &closure_order {
        owners.push(CheckedBody { owner: typeck.def, kind: OwnerKind::Closure(crate::ast::ExprId(c)) });
    }
    let Some(item) = CheckedBody::item(prog, typeck.def) else {
        let d = prog.def(typeck.def);
        v.unchecked.push(format!("{name}: not a body owner ({:?}, item kind {})", d.kind, match &prog.files[d.file as usize].ast.item(d.item).kind { crate::ast::ItemKind::Static(_, _, init) => format!("static init={}", init.is_some()), _ => "other".into() }));
        return v;
    };
    owners.push(item);
    let g = Group { cx, prog, tcx, typeck, env, eval: &eval, root: &root, name: &name, fv_cache };
    check_group(&g, &owners, false, &mut v);
    for child in body::const_children(prog, tcx, typeck) {
        check_group(&g, &[child], true, &mut v);
    }
    v
}

/// `mir_coroutine_witnesses` for each coroutine among `built` (the MIR of the coroutines of one typeck root,
/// analysis MIR after promotion): the regions-erased types of the locals live across a suspension point
/// (`coroutine_hidden_types`, StaticRef and fake-borrow locals ignored for traits), and whether any needs drop.
fn publish_coroutine_witnesses(prog: &Program, tcx: &Tcx, env: TypingEnv, typeck: &typeck::Body, built: &[(CheckedBody, Box<Body>)]) {
    for (owner, mir) in built {
        let (OwnerKind::Closure(e), true) = (owner.kind, mir.coroutine.is_some()) else { continue };
        let witness = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let movable = mir.coroutine.as_ref().is_some_and(|c| c.kind.movability() == crate::mir::Movability::Movable);
            let saved = crate::mir::transform::coroutine::locals_live_across_suspend_points(mir, movable);
            let mut tys: Vec<TyId> = Vec::new();
            for local in saved.iter() {
                let decl = &mir.local_decls[local];
                if matches!(decl.local_info, crate::mir::LocalInfo::StaticRef { is_thread_local: false, .. } | crate::mir::LocalInfo::FakeBorrow) { continue; }
                let ty = tcx.tys.erase_regions(decl.ty);
                if !tys.contains(&ty) { tys.push(ty); }
            }
            let needs_drop = saved.iter().any(|local| tcx.needs_drop(prog, env, mir.local_decls[local].ty));
            (tys, needs_drop)
        }));
        match witness {
            Ok((tys, needs_drop)) => {
                tcx.coroutine_witness_needs_drop.insert((typeck.file, e.0), needs_drop);
                let _k = crate::zone_alloc::kept();
                tcx.coroutine_hidden_types.insert((typeck.file, e.0), std::sync::Arc::new(tys));
            }
            Err(p) => { if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); } }
        }
    }
}

/// rustc `check_coroutine_obligations`: the goals the root's typeck stalled on its coroutines are proven now
/// that the witnesses are known (mode `borrowck`, regions ignored); what does not hold is a normal error.
fn check_coroutine_obligations(prog: &Program, tcx: &Tcx, env: TypingEnv, typeck: &typeck::Body, v: &mut Verdict) {
    use crate::next_solver::fulfill::{NextSolverError, ObligationCause, ObligationCtxt};
    let goals = tcx.coroutine_stalled_predicates.lock().unwrap_or_else(|e| e.into_inner()).remove(&typeck.def);
    let Some(goals) = goals else { return };
    let infcx = crate::infer::InferCtxt::for_item(tcx, prog, env.mode, typeck.def);
    let mut ocx = ObligationCtxt::new(&infcx, prog);
    for (goal, span) in goals {
        ocx.cause = ObligationCause { span, body_id: typeck.def };
        ocx.register_obligation(goal);
    }
    for error in ocx.engine.evaluate_obligations_error_on_ambiguity(&infcx, prog) {
        let (code, o) = match error {
            NextSolverError::TrueError(o) => ("E0277", o),
            NextSolverError::Ambiguity(o) => ("E0283", o),
            NextSolverError::Overflow(o) => ("E0275", o),
        };
        if crate::wfcheck::obligations::predicate_mentions_error(&infcx, &o.goal.predicate) { continue; }
        let message = match &o.goal.predicate.kind {
            crate::next_solver::PredicateKind::Clause(crate::next_solver::ClauseKind::Trait(p)) => {
                let message = typeck::trait_error_message(prog, tcx, code, p, &mut |t| typeck::ty_to_string(false, prog, tcx, t));
                // rustc_on_unimplemented on `Send`/`Sync` for a future: "future cannot be sent between threads safely"
                if message.ends_with("cannot be sent between threads safely") && matches!(p.trait_ref.args.first(), Some(crate::consts::GenericArg::Type(t)) if tcx.is_coroutine(*t)) {
                    "future cannot be sent between threads safely".to_string()
                } else { message }
            }
            predicate => crate::wfcheck::obligations::fulfillment_message(&infcx, prog, code, &o.goal).unwrap_or_else(|| format!("unsatisfied obligation {predicate:?}")),
        };
        let span = Span { file: typeck.file, lo: o.cause.span, hi: o.cause.span };
        let mut d = diagnostics::Diag::new(code, span, message);
        d.labels.push((span, String::new(), true));
        v.errors.push(diagnostics::capture(prog, typeck.def, &d));
    }
}

thread_local! { static WITNESS_IN_FLIGHT: std::cell::RefCell<Vec<DefId>> = const { std::cell::RefCell::new(Vec::new()) }; }

/// The witnesses of the coroutines of `owner`'s body, made on demand (rustc's `mir_coroutine_witnesses`
/// query): the owner's coroutine bodies are built and promoted without being borrow-checked. A body that is
/// already being built further out (a cycle through its own coroutine) publishes nothing.
pub fn ensure_coroutine_witnesses(prog: &Program, tcx: &Tcx, owner: DefId) {
    if WITNESS_IN_FLIGHT.with(|f| f.borrow().contains(&owner)) { return; }
    WITNESS_IN_FLIGHT.with(|f| f.borrow_mut().push(owner));
    let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let typeck = tcx.typeck(prog, owner);
        if !typeck.errors.is_empty() { return; }
        let env = TypingEnv {
            limit: crate::tcx::RecursionLimit::for_item(prog, typeck.def),
            param_env: typeck.param_env.unwrap_or_default(),
            mode: crate::tcx::TypingMode::borrowck(tcx, prog, typeck.def),
        };
        let state = crate::tcx::EvalState::new();
        let eval = EvalCx::new(prog, tcx, env, &state);
        let Ok(root) = BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, &typeck) else { return };
        let provider = BuildProvider { prog, tcx, memo: Default::default() };
        let mut built: Vec<(CheckedBody, Box<Body>)> = Vec::new();
        for c in closures_inner_first(prog, tcx, &typeck) {
            if !tcx.coroutines.contains(&(typeck.file, c)) { continue; }
            let owner = CheckedBody { owner: typeck.def, kind: OwnerKind::Closure(crate::ast::ExprId(c)) };
            let Ok(Ok(mir)) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_mir(&root, owner))) else { return };
            built.push((owner, Box::new(mir)));
        }
        for (_, mir) in &mut built {
            if mir_promoted(prog, tcx, env, &provider, &mut **mir).is_err() { return; }
        }
        publish_coroutine_witnesses(prog, tcx, env, &typeck, &built);
    }));
    WITNESS_IN_FLIGHT.with(|f| { f.borrow_mut().pop(); });
    if let Err(p) = done { if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); } }
}

/// What the bodies of one typeck root share.
struct Group<'a, 'b> {
    cx: &'a EvalCx<'a>,
    prog: &'a Program,
    tcx: &'a Tcx,
    typeck: &'a typeck::Body,
    env: TypingEnv,
    eval: &'b EvalCx<'b>,
    root: &'b BuildRoot<'b>,
    name: &'a str,
    fv_cache: Option<&'a Rc<SummaryCx>>,
}

/// Borrow-check a group of bodies together (one inference context each, nested bodies first):
/// the item with its closures, or one anonymous constant.
fn check_group(g: &Group<'_, '_>, owners: &[CheckedBody], anon_const: bool, v: &mut Verdict) {
    let Group { cx, prog, tcx, typeck, env, eval, root, name, fv_cache } = *g;
    let env = env;
    let provider = BuildProvider { prog, tcx, memo: Default::default() };

    // S3/S4 build, S5 promote. Any stop unchecks the root (see the module doc).
    let mut built: Vec<(CheckedBody, Box<Body>)> = Vec::new();
    for &owner in owners.iter() {
        crate::preempt::point();
        let mir = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_mir(&root, owner))) {
            Ok(Ok(mir)) => mir,
            Ok(Err(e)) => { v.unchecked.push(format!("{name}: {}", describe_build_error(&e.reason))); return; }
            Err(p) => {
                if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
                v.unchecked.push(format!("{name}: internal error in the MIR builder: {}", super::lazy::panic_message(&p)));
                return;
            }
        };
        built.push((owner, Box::new(mir)));
    }
    for (owner, mir) in &mut built {
        match mir_promoted(prog, tcx, env, &provider, &mut **mir) {
            Ok(pre) => {
                if crate::lint::enabled() {
                    // (a lint of the promoted MIR, rustc runs it at the end of mir_promoted)
                    let closure = matches!(owner.kind, OwnerKind::Closure(_));
                    if let Err(p) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::lint_tail_drop::run_lint(prog, tcx, env, typeck.def, closure, mir))) {
                        if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
                    }
                }
                for e in pre.const_errors {
                    let mut d = diagnostics::Diag::new(e.code, e.span, e.message);
                    d.labels.push((e.span, String::new(), true));
                    v.errors.push(diagnostics::capture(prog, typeck.def, &d));
                }
            }
            Err(Blocked(why)) => { v.unchecked.push(format!("{name}: {why}")); let _ = owner; return; }
        }
    }

    // Nesting from the MIR itself (which body aggregates which closure), not from source extents: macro
    // expansions give nested closures equal spans. Inner bodies come first, as do_mir_borrowck requires.
    let mut creator: crate::fxhash::HashMap<u32, Option<u32>> = Default::default();
    for (owner, mir) in &built {
        let by = match owner.kind { OwnerKind::Closure(e) => Some(e.0), _ => None };
        let mut created = vec![];
        created_closures(mir, &mut created);
        for c in created { creator.insert(c, by); }
    }
    let depth = |c: u32| { let (mut d, mut at) = (1usize, c); while let Some(Some(p)) = creator.get(&at) { d += 1; at = *p; if d > creator.len() + 1 { break; } } d };
    built.sort_by_key(|(o, _)| std::cmp::Reverse(match o.kind { OwnerKind::Closure(e) => depth(e.0), _ => 0 }));
    let closure_order: Vec<u32> = built.iter().filter_map(|(o, _)| match o.kind { OwnerKind::Closure(e) => Some(e.0), _ => None }).collect();
    // `mir_coroutine_witnesses` of the coroutines of this root: dropck of their creators, auto traits.
    publish_coroutine_witnesses(prog, tcx, env, typeck, &built);
    check_coroutine_obligations(prog, tcx, env, typeck, v);

    // MoveData, signature regions, inference contexts, MIR type check.
    let mut prepared: Vec<Prepared<'_>> = Vec::new();
    let item_sig = if anon_const { Some(SignatureRegions::anon_const(tcx, prog, typeck.def)) } else { SignatureRegions::function(tcx, prog, typeck.def) };
    let Some(item_sig) = item_sig else {
        v.unchecked.push(format!("{name}: no universal regions for the signature"));
        return;
    };
    // Signatures outer-first: a closure's regions are built on its enclosing closure's (nested bodies
    // run inner-first, so this is a pass of its own over the built bodies).
    let mut closure_sigs: crate::fxhash::HashMap<u32, SignatureRegions> = Default::default();
    for &c in closure_order.iter().rev() {
        let Some((_, mir)) = built.iter().find(|(o, _)| matches!(o.kind, OwnerKind::Closure(e) if e.0 == c)) else { continue };
        // A coroutine (async block, async fn body) has no written signature: rustc's
        // `DefiningTy::Coroutine` has the resume argument as its one input and the return type as
        // output (compute_inputs_and_output), both known to its MIR.
        let synthesized;
        let sigs = match (typeck.closure_sigs.get(&c), mir.coroutine.as_ref()) {
            (Some(sigs), _) => sigs,
            (None, Some(co)) => {
                let bound = tcx.tys.intern(crate::types::TyKind::FnPtr(tcx.tys.mk_list(&[co.resume_ty]), mir.local_decls[0].ty, crate::types::FnHeader::RUST, 0));
                synthesized = typeck::ClosureSignatures { bound, liberated: bound, annotation_eq: None };
                &synthesized
            }
            (None, None) => { v.unchecked.push(format!("{name}: closure without a signature")); return; }
        };
        let parent = match creator.get(&c).copied().flatten().and_then(|p| closure_sigs.get(&p)) {
            Some(p) => p.clone(),
            None => item_sig.clone(),
        };
        let env_ty = mir.local_decls.get(1).map_or(tcx.tys.unit, |d| d.ty);
        let plan = super::universal_regions::coroutine_closure_upvars(typeck, c);
        let Some(sig) = SignatureRegions::closure_in(tcx, prog, typeck.def, c, sigs, plan.as_ref().map(|p| (p, env_ty)), parent) else {
            v.unchecked.push(format!("{name}: closure signature is not a function pointer")); return;
        };
        closure_sigs.insert(c, sig.with_closure_env(tcx, typeck.def, c, env_ty));
    }
    for (owner, mut mir) in built {
        let (sig, key, nested) = match owner.kind {
            OwnerKind::Closure(e) => {
                let Some(sig) = closure_sigs.get(&e.0).cloned() else { v.unchecked.push(format!("{name}: closure without a signature")); return; };
                let id = ClosureId { file: typeck.file, expr: e.0 };
                let key = if mir.coroutine.is_some() { ClauseOwner::Coroutine(id) } else { ClauseOwner::Closure(id) };
                (sig, key, true)
            }
            _ => (item_sig.clone(), ClauseOwner::Def(typeck.def), false),
        };
        let moves = MoveData::gather_moves(&mir, prog, tcx, env, &|_| true);
        let mut infcx = BorrowckInferCtxt::new(tcx, prog, typeck.def, typeck.def, &typeck.args);
        let input = MirNllInput { sig: &sig, nested, def_span: mir.span, moves: &moves, has_loans: true };
        let tc = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| type_check_body(tcx, prog, &mut infcx, &mut *mir, &input))) {
            Ok(tc) => tc,
            Err(p) => {
                if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
                v.unchecked.push(format!("{name}: internal error in the MIR type check: {}", super::lazy::panic_message(&p)));
                return;
            }
        };
        prepared.push(Prepared { owner, key, nested, mir, moves, sig, infcx, typeck: Some(tc) });
    }

    // The type check's own findings (E0161 and friends) and its internal-error reports.
    for p in &mut prepared {
        let tc = p.typeck.as_mut().unwrap();
        let errors = std::mem::take(&mut tc.summary.errors);
        for bug in std::mem::take(&mut tc.summary.mir_bugs) {
            v.unchecked.push(format!("{name}: MIR bug: {}", render_ids(prog, tcx, &bug.message)));
        }
        let borrows = BorrowSet::build(tcx, &p.mir, false, &p.moves);
        let regions_stub = None::<()>;
        let _ = regions_stub;
        for e in errors {
            let d = diagnostics::Diag::new(e.code(), e.span, e.kind.note());
            v.errors.push(diagnostics::capture(prog, typeck.def, &d));
            let _ = &borrows;
        }
        // E0320: the drop-check rules of a local's type overflowed the recursion limit
        // (DropckOutlivesResult::report_overflows)
        for o in std::mem::take(&mut tc.summary.dropck_overflows) {
            let show = |t| crate::typeck::ty_to_string(false, prog, tcx, t);
            let mut d = diagnostics::Diag::new("E0320", o.span, format!("overflow while adding drop-check rules for `{}`", show(o.ty)));
            d.note(format!("overflowed on `{}`", show(o.overflow_ty)));
            v.errors.push(diagnostics::capture(prog, typeck.def, &d));
        }
    }

    // Field views of the root (S15): the MIR source of callees.
    let own_cache;
    let fv_cache: &SummaryCx = match fv_cache { Some(c) => c, None => { own_cache = SummaryCx::new(); &own_cache } };
    let bodies_of = MirBodies { prog, tcx };
    let relaxed = field_views::relaxed(prog, typeck.def);
    let closure_fv: crate::fxhash::HashMap<u32, Vec<field_views::Summary>> = {
        let fenv = field_views::FvEnv { prog, tcx, cx: eval, cache: fv_cache, bodies: &bodies_of };
        prepared.iter().filter_map(|p| match p.owner.kind {
            OwnerKind::Closure(e) => Some((e.0, field_views::closure_captures(&fenv, &p.mir))),
            _ => None,
        }).collect()
    };
    let views = FieldViews::new(prog, env, cx.state, fv_cache, &bodies_of, relaxed, closure_fv);

    // do_mir_borrowck: nested bodies first.
    let results = {
        let root_bodies: Vec<RootBody<'_>> = prepared.iter_mut().map(|p| RootBody {
            key: p.key, body: &p.mir, moves: &p.moves, infcx: &mut p.infcx as &mut dyn RootBodyOps,
            typeck: p.typeck.take().unwrap(), nested: p.nested,
        }).collect();
        let ctxt = BorrowCheckRootCtxt { tcx, prog };
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctxt.do_mir_borrowck_fv(root_bodies, Some(&views)))) {
            Ok(r) => r,
            Err(p) => {
                if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
                v.unchecked.push(format!("{name}: internal error in the borrow checker: {}", super::lazy::panic_message(&p)));
                return;
            }
        }
    };

    // diagnostics
    for (p, checked) in prepared.iter().zip(results.bodies.iter()) {
        v.bodies += 1;
        if let Some(why) = checked.result.unsupported.first() {
            v.unchecked.push(format!("{name}: {why}"));
            continue;
        }
        let borrows = BorrowSet::build(tcx, &p.mir, matches!(p.mir.source.kind, BodyKind::Fn | BodyKind::Closure(_)), &p.moves);
        let names = universal_names(tcx, prog, p);
        let mut dcx = DiagCx::new(tcx, prog, p.owner.owner, &p.mir, &p.moves, &checked.regions, &borrows, &names);
        dcx.coroutine_kinds = checked.coroutine_kinds.clone();
        if crate::lint::enabled() && checked.result.accepted() && matches!(p.owner.kind, OwnerKind::Function | OwnerKind::Closure(_)) {
            lint_unused_mut(prog, typeck.def, &p.mir, &checked.result.used_mut);
        }
        v.errors.extend(diagnostics::render_errors(&dcx, &checked.result));
        // `report_opaque_type_errors`: only when the region inference found no error (those would explain them).
        if checked.regions.regioncx.errors.is_empty() {
            v.errors.extend(diagnostics::opaque_types::report_opaque_type_errors(&dcx, &checked.deferred_opaque_type_errors).iter().map(|d| diagnostics::capture(prog, p.owner.owner, d)));
        }
        v.fv_only.extend(diagnostics::render_fv_only(&dcx, &checked.result));
        v.fv_records.extend(checked.result.fv_records.iter().cloned());
        v.relaxed_records.extend(checked.result.relaxed_records.iter().cloned());
    }
}

/// A MIR-bug text with its interned type ids (`TyId(12)`) written out as types, so the unchecked
/// histogram says which types the failing operation was about.
fn render_ids(prog: &Program, tcx: &Tcx, text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("TyId(") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 5..];
        match tail.find(')').and_then(|j| tail[..j].parse::<u32>().ok().map(|n| (j, n))) {
            Some((j, n)) => { out.push_str(&crate::typeck::ty_to_string(false, prog, tcx, TyId(n))); rest = &tail[j + 1..]; }
            None => { out.push_str("TyId("); rest = tail; }
        }
    }
    out.push_str(rest);
    out
}

fn describe_build_error(reason: &crate::mir::build::UnsupportedReason) -> String {
    use crate::mir::build::UnsupportedReason::*;
    match reason {
        CustomMir => "custom_mir body".into(),
        CVariadic => "C-variadic body".into(),
        InlineAsm => "inline asm".into(),
        Reborrow => "reborrow".into(),
        UnsafeBinder => "unsafe binder".into(),
        AsyncDrop => "async drop".into(),
        TypeckTainted => "typeck reported errors".into(),
        Internal(w) => format!("internal MIR builder error: {w}"),
        UnresolvedFact(f) => format!("unresolved typeck fact: {f}"),
        NotYetBuilt(w) => format!("MIR builder: {w} not built"),
    }
}

/// The universal-region names of a body (DESIGN-C-NLL5 contract 3), from its signature.
fn universal_names(tcx: &Tcx, prog: &Program, p: &Prepared<'_>) -> UniversalNames {
    let mut names = UniversalNames::default();
    let uni_vid = |r| p.typeck_vid(r);
    for (i, u) in p.sig.regions.iter().enumerate() {
        let vid = match u.identity { Identity::Region(_, r) => uni_vid(r), _ => Some(i) };
        let Some(vid) = vid else { continue };
        let ext = match u.identity {
            Identity::Static => ExternalName::Static,
            Identity::Region(scope, r) => match tcx.tys.region_kind(r) {
                crate::types::RegionKind::ReLateParam(_, crate::types::LateParamKind::ClosureEnv) =>
                    ExternalName::ClosureEnv { span: Span::default(), closure_trait: "Fn" },
                crate::types::RegionKind::ReEarlyParam(_) | crate::types::RegionKind::ReLateParam(_, crate::types::LateParamKind::Named(_)) if u.name != "'_" =>
                    ExternalName::Named { name: u.name.clone(), span: Span::default(), early: matches!(tcx.tys.region_kind(r), crate::types::RegionKind::ReEarlyParam(_)) },
                _ => { let _ = scope; ExternalName::Anon }
            },
            Identity::FnBody(_) => continue,
        };
        names.regions.push((vid, ext));
    }
    // The signature's types name its universal regions by their variables, as the region names are looked up.
    let by_vid = |ty| tcx.tys.map_regions(ty, &|r, _| match uni_vid(r) { Some(v) => tcx.tys.intern_region(crate::types::RegionKind::ReVar(v as u32)), None => r }, 0);
    for (i, &ty) in p.sig.inputs.iter().enumerate() {
        let ty = by_vid(ty);
        let span = p.mir.local_decls.get(i + 1).map_or(Span::default(), |d| d.source_info.span);
        names.inputs.push(InputInfo { ty, ty_span: span, ref_span: matches!(tcx.tys.kind(ty), crate::types::TyKind::Ref(..)).then_some(span) });
    }
    names.output = Some(OutputInfo { ty: by_vid(p.sig.output), span: p.mir.span, ref_span: None, description: "" });
    names.def_descr = match p.owner.kind { OwnerKind::Closure(_) => "closure", OwnerKind::Function => "function", _ => "constant" };
    names.is_fn_def = matches!(p.owner.kind, OwnerKind::Function);
    names.is_const = matches!(p.owner.kind, OwnerKind::Const | OwnerKind::Static | OwnerKind::AssocConst | OwnerKind::InlineConst(_) | OwnerKind::AnonConst(_));
    let _ = prog;
    names
}

impl Prepared<'_> {
    /// The region variable of the universal region named `r` (signature order = variable order).
    fn typeck_vid(&self, r: crate::types::RegionId) -> Option<usize> {
        self.sig.regions.iter().position(|u| matches!(u.identity, Identity::Region(_, x) if x == r))
    }
}

/// The analysis MIR of callee instances for field-view summaries: typeck instantiate, build,
/// promote. A callee whose MIR cannot be produced has an unknown summary (the whole borrow).
pub struct MirBodies<'a> { prog: &'a Program, tcx: &'a Tcx }
impl<'a> MirBodies<'a> { pub fn new(prog: &'a Program, tcx: &'a Tcx) -> Self { Self { prog, tcx } } }
impl FvBodies for MirBodies<'_> {
    fn body_of(&self, cx: &EvalCx<'_>, tcx: &Tcx, d: DefId, args: &[TyId]) -> Option<FvBody> {
        let (prog, _) = (self.prog, self.tcx);
        let body = typeck::try_instantiate_body(cx, prog, tcx, d, args).ok()?;
        if !body.errors.is_empty() || body.body.is_none() { return None; }
        let env = TypingEnv {
            limit: crate::tcx::RecursionLimit::for_item(prog, body.def),
            param_env: body.param_env.unwrap_or_default(),
            mode: crate::tcx::TypingMode::borrowck(tcx, prog, body.def),
        };
        let eval = EvalCx::new(prog, tcx, env, cx.state);
        let root = BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, &body).ok()?;
        let owner = CheckedBody::item(prog, d)?;
        let mut mir = build_mir(&root, owner).ok()?;
        mir_promoted(prog, tcx, env, &BuildProvider { prog, tcx, memo: Default::default() }, &mut mir).ok()?;
        Some(FvBody { body: mir, env })
    }
}

#[allow(dead_code)]
fn _events(_: &event::BorrowError) {}

/// An anonymous constant that is a body owner of its own is const-checked alone (S5).
pub fn check_anon_const(cx: &EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &typeck::Body, expr: crate::ast::ExprId) -> Vec<event::BorrowError> {
    let env = TypingEnv {
        limit: crate::tcx::RecursionLimit::for_item(prog, body.def),
        param_env: body.param_env.unwrap_or_default(),
        mode: crate::tcx::TypingMode::borrowck(tcx, prog, body.def),
    };
    let eval = EvalCx::new(prog, tcx, env, cx.state);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let root = BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, body).ok()?;
        let mut mir = build_mir(&root, CheckedBody { owner: body.def, kind: OwnerKind::AnonConst(expr) }).ok()?;
        let pre = mir_promoted(prog, tcx, env, &BuildProvider { prog, tcx, memo: Default::default() }, &mut mir).ok()?;
        Some(pre.const_errors.into_iter().map(|e| {
            let mut d = diagnostics::Diag::new(e.code, e.span, e.message);
            d.labels.push((e.span, String::new(), true));
            diagnostics::capture(prog, body.def, &d)
        }).collect::<Vec<_>>())
    }));
    match result {
        Ok(Some(errors)) => errors,
        Ok(None) => Vec::new(),
        Err(p) => { if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); } Vec::new() }
    }
}

/// rustc_borrowck lib.rs lint_unused_mut: a user binding or argument declared `mut` that no write or
/// mutable borrow needed -- `unused_mut`. The pattern span of the binding says where the `mut` is.
fn lint_unused_mut(prog: &Program, owner: DefId, mir: &Body, used: &crate::mir::dataflow::BitSet) {
    use crate::mir::{BindingForm, ByRef, LocalInfo, LocalKind, Mutability, VarBindingForm};
    for (i, decl) in mir.local_decls.iter().enumerate() {
        // (the resume argument of a coroutine is `_task_context`: a name with a leading underscore is never linted)
        if mir.coroutine.is_some() && i == 2 { continue; }
        if decl.mutability != Mutability::Mut || used.contains(i) || !matches!(decl.kind, LocalKind::UserVar | LocalKind::Arg) { continue; }
        let LocalInfo::User(BindingForm::Var(VarBindingForm { binding_mode: crate::mir::BindingMode(ByRef::No, Mutability::Mut), pat_span, .. })) = &decl.local_info else { continue };
        let Some(name) = mir.var_debug_info.iter().find(|v| v.place.local.0 as usize == i && v.place.projection.is_empty()).map(|v| v.name.as_str()) else { continue };
        if name.starts_with('_') { continue; }
        crate::lint::emit(prog, owner, &crate::lint::UNUSED_MUT, pat_span.file, pat_span.lo,
            &format!("variable does not need to be mutable -- help: remove this `mut` (`{}`)", name));
    }
}
