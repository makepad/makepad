//! `MAKEPAD_RUN_MIR_BORROWCK_ALL=1 makepad check <pkg>`: the whole MIR borrowck pipeline on every body root of the
//! package's dependency graph, the way S11 will call it: build, `mir_promoted`, signature regions, `type_check_body`
//! (renumber, MIR type check, liveness), `do_mir_borrowck` (nested bodies first, region inference, `mir_borrowck`).
//! Prints the errors in `BORROWCK_ALL`'s format and an inventory (errors per code, MIR bugs, unchecked bodies by reason)
//! so it can be diffed against the BIR checker's, app by app.
//!
//! `MAKEPAD_RUN_MIR_BUILD_ONLY=<substr>` restricts the owners (as for the build harness).
use super::driver::closure_children;
use super::{build_mir, BuildCx, BuildRoot};
use crate::ast::ExprKind;
use crate::borrowck::body::{self, BodyKind, CheckedBody};
use crate::borrowck::infer_ctxt::BorrowckInferCtxt;
use crate::borrowck::mir_nll::{type_check_body, MirNllInput};
use crate::borrowck::root_cx::{BorrowCheckRootCtxt, RootBody};
use crate::borrowck::type_check::ClauseOwner;
use crate::borrowck::universal_regions::SignatureRegions;
use crate::mir::move_paths::MoveData;
use crate::mir::{Body, ClosureId, Span};
use crate::program::Program;
use crate::tcx::{EvalCx, EvalState, RecursionLimit, Tcx, TypingEnv, TypingMode};
use std::collections::BTreeMap;

#[derive(Default)]
struct Inventory {
    rows: BTreeMap<String, (usize, String)>,
}
impl Inventory {
    fn note(&mut self, key: impl Into<String>, at: impl Into<String>) {
        let e = self.rows.entry(key.into()).or_insert((0, at.into()));
        e.0 += 1;
    }
    fn total(&self) -> usize {
        self.rows.values().map(|r| r.0).sum()
    }
    fn print(&self, title: &str) {
        let mut rows: Vec<_> = self.rows.iter().collect();
        rows.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (key, (n, at)) in rows {
            eprintln!("makepad-run mir-borrowck: {title} {n:>6}  {key}   e.g. {at}");
        }
    }
}

fn line_of(prog: &Program, file: u32, pos: u32) -> usize {
    let src = prog.src(file).src;
    1 + src[..(pos as usize).min(src.len())].iter().filter(|&&c| c == b'\n').count()
}

/// Returns the number of borrowck errors reported (the gate).
pub fn borrowck_all(prog: &Program, tcx: &Tcx, with_std: bool) -> usize {
    let only = std::env::var("MAKEPAD_RUN_MIR_BUILD_ONLY").ok();
    let std_crates: Vec<u32> = ["core", "alloc", "std"].iter().filter_map(|n| prog.crates.iter().position(|c| prog.syms.str(c.name) == *n).map(|i| i as u32)).collect();
    let (mut roots, mut checked_bodies, mut errors_total, mut typeck_fail) = (0usize, 0usize, 0usize, 0usize);
    let (mut errors, mut region_errors, mut mir_bugs, mut unchecked, mut panics) =
        (Inventory::default(), Inventory::default(), Inventory::default(), Inventory::default(), Inventory::default());
    let t0 = relax_backend::cpu_ns();
    for owner in body::owners(prog) {
        let d = prog.def(owner.owner);
        if !with_std && std_crates.contains(&d.krate) {
            continue;
        }
        let path = prog.def_path(owner.owner);
        if let Some(o) = &only {
            if !path.contains(o.as_str()) {
                continue;
            }
        }
        let typeck_body = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tcx.typeck(prog, owner.owner))) {
            Ok(b) if b.errors.is_empty() => b,
            _ => {
                typeck_fail += 1;
                continue;
            }
        };
        roots += 1;
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_root(prog, tcx, owner, &typeck_body)));
        match r {
            Ok(outcome) => {
                checked_bodies += outcome.checked;
                for (reason, at) in outcome.unchecked {
                    unchecked.note(reason, at);
                }
                for (key, at) in outcome.mir_bugs {
                    mir_bugs.note(key, at);
                }
                for (key, at) in outcome.region_errors {
                    region_errors.note(key, at);
                }
                for (code, line, at) in outcome.errors {
                    println!("error[{code}]: {line}");
                    errors.note(code, at);
                    errors_total += 1;
                }
            }
            Err(p) => {
                if crate::preempt::is_cancelled(&*p) {
                    std::panic::resume_unwind(p);
                }
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
                let (key, detail) = match msg.split_once(": ") { Some((k, v)) => (k.to_string(), format!(" [{v}]")), None => (msg.clone(), String::new()) };
                panics.note(key, format!("{path}{detail}"));
            }
        }
    }
    eprintln!(
        "makepad-run mir-borrowck: {roots} roots ({typeck_fail} failed typeck), {checked_bodies} bodies checked, {errors_total} errors, {} region errors, {} MIR bugs, {} unchecked, {} panicked; cpu {:.1} ms",
        region_errors.total(), mir_bugs.total(), unchecked.total(), panics.total(), (relax_backend::cpu_ns() - t0) as f64 / 1e6
    );
    errors.print("ERROR");
    region_errors.print("REGION ERROR");
    mir_bugs.print("MIR BUG");
    unchecked.print("unchecked");
    panics.print("PANIC");
    errors_total + panics.total()
}

#[derive(Default)]
struct RootOutcome {
    checked: usize,
    /// (code, rendered line, example)
    errors: Vec<(String, String, String)>,
    region_errors: Vec<(String, String)>,
    mir_bugs: Vec<(String, String)>,
    unchecked: Vec<(String, String)>,
}

fn check_root(prog: &Program, tcx: &Tcx, owner: CheckedBody, tb: &crate::typeck::Body) -> RootOutcome {
    let mut out = RootOutcome::default();
    let path = prog.def_path(owner.owner);
    let env = TypingEnv { limit: RecursionLimit::for_item(prog, tb.def), param_env: tb.param_env.unwrap_or_default(), mode: TypingMode::borrowck(tcx, prog, tb.def) };
    let state = EvalState::new();
    let eval = EvalCx::new(prog, tcx, env, &state);
    let root = match BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, tb) {
        Ok(root) => root,
        Err(why) => {
            out.unchecked.push((format!("no region scope tree: {why}"), path));
            return out;
        }
    };
    // Nested bodies before their parents: inner closures (smaller spans) first, the owner last.
    let ast = &prog.files[tb.file as usize].ast;
    let mut nested = closure_children(prog, tcx, tb);
    let span_of = |b: &CheckedBody| if let BodyKind::Closure(e) = b.kind { let x = ast.expr(e); (x.lo, x.hi) } else { (0, 0) };
    nested.sort_by_key(|b| { let (lo, hi) = span_of(b); (hi - lo, lo) });
    let owner_sig = match SignatureRegions::function(tcx, prog, tb.def) {
        Some(sig) => sig,
        None => {
            out.unchecked.push(("no native signature for the owner".into(), path));
            return out;
        }
    };
    struct Prepared {
        key: ClauseOwner,
        mir: Body,
        moves: MoveData,
        infcx: BorrowckInferCtxt<'static>,
        typeck: Option<crate::borrowck::mir_nll::MirTypeck>,
        nested: bool,
        at: String,
    }
    let mut prepared: Vec<Prepared> = Vec::new();
    let provider = super::driver::BuildProvider { prog, tcx, memo: Default::default() };
    let mut all: Vec<CheckedBody> = nested.clone();
    all.push(owner);
    for b in all {
        let at = if let BodyKind::Closure(e) = b.kind { format!("{path}::{{closure#{}}} ({}:{})", e.0, prog.file_paths[tb.file as usize], line_of(prog, tb.file, ast.expr(e).lo)) } else { path.clone() };
        let mut mir = match build_mir(&root, b) {
            Ok(m) => m,
            Err(err) => {
                // A body that cannot be built leaves its whole root unchecked (nested bodies depend on each other).
                out.unchecked.push((format!("not built: {:?}", err.reason), at));
                return out;
            }
        };
        if let Err(why) = crate::mir::transform::mir_promoted(prog, tcx, env, &provider, &mut mir) {
            out.unchecked.push((format!("blocked: {}", why.0), at));
            return out;
        }
        // The signature regions of this body.
        let closure = if let BodyKind::Closure(e) = b.kind { Some(e) } else { None };
        let sig = match closure {
            None => owner_sig.clone(),
            Some(e) => {
                let Some(sigs) = tb.closure_sigs.get(&e.0) else {
                    out.unchecked.push(("closure without a recorded signature".into(), at));
                    return out;
                };
                // The enclosing closure (innermost other closure containing this one), else the owner.
                let (lo, hi) = span_of(&b);
                let parent = nested.iter().filter(|p| **p != b).filter(|p| { let (plo, phi) = span_of(p); plo <= lo && hi <= phi && (plo, phi) != (lo, hi) })
                    .min_by_key(|p| { let (plo, phi) = span_of(p); phi - plo });
                let parent_sig = match parent {
                    None => owner_sig.clone(),
                    Some(p) => {
                        let BodyKind::Closure(pe) = p.kind else { unreachable!() };
                        let Some(psigs) = tb.closure_sigs.get(&pe.0) else {
                            out.unchecked.push(("enclosing closure without a recorded signature".into(), at));
                            return out;
                        };
                        match SignatureRegions::closure_in(tcx, prog, tb.def, pe.0, psigs, None, owner_sig.clone()) {
                            Some(s) => s,
                            None => {
                                out.unchecked.push(("enclosing closure has no signature regions".into(), at));
                                return out;
                            }
                        }
                    }
                };
                let plan = crate::borrowck::universal_regions::coroutine_closure_upvars(tb, e.0);
                let Some(sig) = SignatureRegions::closure_in(tcx, prog, tb.def, e.0, sigs, plan.as_ref().map(|p| (p, mir.local_decls[1].ty)), parent_sig) else {
                    out.unchecked.push(("closure has no signature regions".into(), at));
                    return out;
                };
                sig.with_closure_env(tcx, tb.def, e.0, mir.local_decls[1].ty)
            }
        };
        // rustc_borrowck: `MoveData::gather_moves(body, tcx, |_| true)`.
        let moves = MoveData::gather_moves(&mir, prog, tcx, env, &|_| true);
        let has_loans = mir.basic_blocks.iter().flat_map(|b| &b.statements).any(|s| matches!(&s.kind, crate::mir::StatementKind::Assign(_, crate::mir::Rvalue::Ref(..))));
        // SAFETY: the inference context borrows only the program and type context, which outlive this function;
        // the 'static is a lifetime erasure for the Vec below.
        let (prog_s, tcx_s): (&'static Program, &'static Tcx) = unsafe { (std::mem::transmute(prog), std::mem::transmute(tcx)) };
        let mut infcx = BorrowckInferCtxt::new(tcx_s, prog_s, tb.def, tb.def, &tb.args);
        let item_lo = prog.files[tb.file as usize].ast.item(prog.def(tb.def).item).lo;
        let def_span = Span { file: tb.file, lo: item_lo, hi: item_lo };
        let nested_flag = closure.is_some();
        let typeck = {
            let input = MirNllInput { sig: &sig, nested: nested_flag, def_span, moves: &moves, has_loans };
            type_check_body(tcx, prog, &mut infcx, &mut mir, &input)
        };
        for bug in &typeck.summary.mir_bugs {
            out.mir_bugs.push((bug.message.split(['(', ':']).next().unwrap_or("").trim().to_string(), format!("{at}: {}", bug.message)));
        }
        let key = match closure {
            None => ClauseOwner::Def(tb.def),
            Some(e) => {
                let id = ClosureId { file: tb.file, expr: e.0 };
                if matches!(ast.expr(e).kind, ExprKind::Async(..)) { ClauseOwner::Coroutine(id) } else { ClauseOwner::Closure(id) }
            }
        };
        prepared.push(Prepared { key, mir, moves, infcx, typeck: Some(typeck), nested: nested_flag, at });
    }
    let ats: Vec<String> = prepared.iter().map(|p| p.at.clone()).collect();
    let mut items = prepared;
    let bodies: Vec<RootBody<'_>> = items.iter_mut().map(|p| RootBody { key: p.key, body: unsafe { &*(&p.mir as *const Body) }, moves: unsafe { &*(&p.moves as *const MoveData) }, infcx: &mut p.infcx, typeck: p.typeck.take().unwrap(), nested: p.nested }).collect();
    let result = BorrowCheckRootCtxt { tcx, prog }.do_mir_borrowck(bodies);
    out.checked = result.bodies.len();
    for (checked, at) in result.bodies.iter().zip(&ats) {
        let r = &checked.result;
        for why in &r.unsupported {
            out.unchecked.push((format!("unsupported: {why}"), at.clone()));
        }
        for e in &r.errors {
            let line = line_of(prog, tb.file, e.span.lo);
            out.errors.push((e.code().to_string(), format!("{} {}:{}", e.kind.code(), prog.file_paths[tb.file as usize], line), at.clone()));
        }
        for e in &r.region_errors {
            out.region_errors.push((format!("{e:?}").split(['(', ' ', '{']).next().unwrap_or("").to_string(), at.clone()));
        }
    }
    out
}
