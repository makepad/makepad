//! The MIR-building harness behind `MAKEPAD_RUN_MIR_BUILD_ALL=1 makepad check <pkg>`: build the MIR of every body
//! owner (fns, consts, statics, anonymous consts, closures and async blocks), run `mir::validate` on each result
//! and print a per-reason histogram of the bodies that could not be built or do not validate.
//!
//! `MAKEPAD_RUN_MIR_BUILD_ALL=std` includes the standard library crates, `MAKEPAD_RUN_MIR_BUILD_ONLY=<substr>`
//! restricts the owners to those whose path contains the substring, and `MAKEPAD_RUN_MIR_BUILD_DUMP=<substr>`
//! prints the pretty MIR (`mir::pretty`) of each built body whose path contains the substring.
use super::{build_mir, BuildCx, BuildRoot, UnsupportedReason};
use crate::ast::{ExprId, ExprKind, ItemKind as ExprKindItem};
use crate::borrowck::body::{self, BodyKind, CheckedBody};
use crate::program::Program;
use crate::tcx::{EvalCx, EvalState, RecursionLimit, Tcx, TypingEnv, TypingMode};
use crate::typeck;
use std::collections::BTreeMap;

/// The closure-like children of a typeck root: closures and async blocks whose expression facts this root owns.
pub fn closure_children(prog: &Program, tcx: &Tcx, body: &typeck::Body) -> Vec<CheckedBody> {
    let ast = &prog.files[body.file as usize].ast;
    let mut out = Vec::new();
    // An async fn's body is a coroutine of its own, identified by `ASYNC_FN_BODY | body block`.
    if let ExprKindItem::Fn(sig, _, _) = ast.item(prog.def(body.def).item).kind {
        if let (true, Some(block)) = (ast.fn_sig(sig).asyncness, body.body) {
            out.push(CheckedBody { owner: body.def, kind: BodyKind::Closure(ExprId(crate::tcx::ASYNC_FN_BODY | block.0)) });
        }
    }
    for i in 0..body.expr_ty.len() {
        if body.expr_ty[i] == tcx.tys.error {
            continue;
        }
        let e = ExprId(if i < body.expr_n1 as usize { body.expr_lo + i as u32 } else { body.expr_lo2 + i as u32 - body.expr_n1 });
        match ast.expr(e).kind {
            ExprKind::Closure(_) => out.push(CheckedBody { owner: body.def, kind: BodyKind::Closure(e) }),
            // Async blocks and an async closure's coroutine are bodies of their own.
            ExprKind::Async(..) => out.push(CheckedBody { owner: body.def, kind: BodyKind::Closure(e) }),
            _ => {}
        }
    }
    out
}

fn line_of(prog: &Program, file: u32, pos: u32) -> usize {
    let src = prog.src(file).src;
    1 + src[..(pos as usize).min(src.len())].iter().filter(|&&c| c == b'\n').count()
}

/// `mir_const_qualif` of another constant's value (S11: use this as the `ConstQualifsProvider`, construct with `Default` memo): build its MIR and const-check it (memoised; a cycle blocks).
pub struct BuildProvider<'a> {
    pub prog: &'a Program,
    pub tcx: &'a Tcx,
    pub memo: std::cell::RefCell<std::collections::HashMap<crate::program::DefId, Result<crate::mir::transform::qualifs::ConstQualifs, crate::mir::transform::qualifs::Blocked>>>,
}
impl crate::mir::transform::qualifs::ConstQualifsProvider for BuildProvider<'_> {
    fn mir_const_qualif(&self, def: crate::program::DefId) -> Result<crate::mir::transform::qualifs::ConstQualifs, crate::mir::transform::qualifs::Blocked> {
        use crate::mir::transform::qualifs::Blocked;
        if let Some(r) = self.memo.borrow().get(&def) {
            return r.clone();
        }
        // In progress: a cycle blocks.
        self.memo.borrow_mut().insert(def, Err(Blocked("constant qualification cycle")));
        let result = (|| {
            // A constant item, or an anonymous constant of a body (a repeat count, a `const {}` operand):
            // the latter is built from its enclosing item's typeck facts.
            let (owner, typeck_root) = match CheckedBody::item(self.prog, def) {
                Some(o) => (o, def),
                None => {
                    use crate::def_collector::NodeId;
                    let kind = match self.prog.def_id_to_node_id.get(&def) {
                        Some(&(_, NodeId::AnonConst(e))) => BodyKind::AnonConst(e),
                        Some(&(_, NodeId::InlineConst(e))) => BodyKind::InlineConst(e),
                        _ => return Err(Blocked("not a body owner")),
                    };
                    let mut root = self.prog.def(def).parent;
                    while matches!(self.prog.def(root).kind, crate::program::DefKind::Closure | crate::program::DefKind::AnonConst | crate::program::DefKind::OpaqueTy) { root = self.prog.def(root).parent; }
                    (CheckedBody { owner: root, kind }, root)
                }
            };
            let typeck_body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tcx.typeck(self.prog, typeck_root))).map_err(|_| Blocked("typeck panicked"))?;
            if !typeck_body.errors.is_empty() {
                return Err(Blocked("typeck errors"));
            }
            let env = TypingEnv {
                limit: RecursionLimit::for_item(self.prog, typeck_body.def),
                param_env: typeck_body.param_env.unwrap_or_default(),
                mode: TypingMode::borrowck(self.tcx, self.prog, typeck_body.def),
            };
            let state = EvalState::new();
            let eval = EvalCx::new(self.prog, self.tcx, env, &state);
            let root = BuildRoot::new(BuildCx { prog: self.prog, tcx: self.tcx, env, eval: &eval }, &typeck_body).map_err(|_| Blocked("no scope tree"))?;
            let mir = build_mir(&root, owner).map_err(|_| Blocked("constant body not built"))?;
            let ccx = crate::mir::transform::const_check::ConstCx::new(self.prog, self.tcx, env, &mir, self);
            crate::mir::transform::const_check::mir_const_qualif(&ccx).map(|c| c.qualifs)
        })();
        self.memo.borrow_mut().insert(def, result.clone());
        result
    }
}

#[derive(Default)]
struct Histogram {
    counts: BTreeMap<String, usize>,
    example: BTreeMap<String, String>,
}
impl Histogram {
    fn note(&mut self, reason: String, at: String) {
        *self.counts.entry(reason.clone()).or_insert(0) += 1;
        self.example.entry(reason).or_insert(at);
    }
}

fn reason_key(reason: &UnsupportedReason) -> String {
    match reason {
        UnsupportedReason::NotYetBuilt(what) => format!("not yet built: {what}"),
        UnsupportedReason::Internal(what) => format!("internal: {what}"),
        UnsupportedReason::UnresolvedFact(what) => format!("unresolved fact: {what}"),
        other => format!("{other:?}"),
    }
}

/// Build and validate every body. Returns the number of bodies that panicked or did not validate (the gate);
/// unbuilt bodies are counted per reason and are not failures.
pub fn build_all(prog: &Program, tcx: &Tcx, with_std: bool) -> usize {
    let only = std::env::var("MAKEPAD_RUN_MIR_BUILD_ONLY").ok();
    let dump: Vec<String> = std::env::var("MAKEPAD_RUN_MIR_BUILD_DUMP").map(|s| s.split(',').map(str::to_string).collect()).unwrap_or_default();
    let std_crates: Vec<u32> = ["core", "alloc", "std"].iter().filter_map(|n| prog.crates.iter().position(|c| prog.syms.str(c.name) == *n).map(|i| i as u32)).collect();
    let (mut owners, mut typeck_fail, mut built, mut validate_fail, mut panics) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut unbuilt = Histogram::default();
    let mut invalid = Histogram::default();
    let mut panicked = Histogram::default();
    let mut blocked = Histogram::default();
    let mut const_errors = Histogram::default();
    let (mut promoted_ok, mut promoted_invalid, mut n_promoted) = (0usize, 0usize, 0usize);
    let provider = BuildProvider { prog, tcx, memo: Default::default() };
    let passes = std::env::var_os("MAKEPAD_RUN_MIR_BUILD_NO_PASSES").is_none();
    let mut by_kind: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
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
        owners += 1;
        let typeck_body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tcx.typeck(prog, owner.owner)));
        let typeck_body = match typeck_body {
            Ok(b) if b.errors.is_empty() => b,
            _ => {
                typeck_fail += 1;
                continue;
            }
        };
        let env = TypingEnv {
            limit: RecursionLimit::for_item(prog, typeck_body.def),
            param_env: typeck_body.param_env.unwrap_or_default(),
            mode: TypingMode::borrowck(tcx, prog, typeck_body.def),
        };
        let state = EvalState::new();
        let eval = EvalCx::new(prog, tcx, env, &state);
        let root = match BuildRoot::new(BuildCx { prog, tcx, env, eval: &eval }, &typeck_body) {
            Ok(root) => root,
            Err(why) => {
                unbuilt.note(format!("no region scope tree: {why}"), path);
                continue;
            }
        };
        let mut bodies = vec![owner];
        bodies.extend(body::const_children(prog, tcx, &typeck_body));
        bodies.extend(closure_children(prog, tcx, &typeck_body));
        for b in bodies {
            let kind = super::owner_kind_name(b.kind);
            let slot = by_kind.entry(kind).or_insert((0, 0));
            slot.0 += 1;
            let mut loc = String::new();
            let at = if let BodyKind::Closure(e) = b.kind {
                let ast = &prog.files[typeck_body.file as usize].ast;
                let lo = if e.0 & crate::tcx::ASYNC_FN_BODY != 0 { ast.block(crate::ast::BlockId(e.0 & !crate::tcx::ASYNC_FN_BODY)).lo } else { ast.expr(e).lo };
                loc = format!(" ({}:{})", prog.file_paths[typeck_body.file as usize], line_of(prog, typeck_body.file, lo));
                format!("{path}::{{closure#{}}}", e.0)
            } else {
                path.clone()
            };
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_mir(&root, b))) {
                Ok(Ok(mir)) => {
                    built += 1;
                    slot.1 += 1;
                    if dump.iter().any(|s| at.contains(s.as_str())) {
                        println!("{}", crate::mir::pretty::print_body(prog, tcx, &at, &mir));
                    }
                    let errors = crate::mir::validate::validate(&mir, tcx);
                    if let Some(first) = errors.first() {
                        validate_fail += 1;
                        invalid.note(format!("{} (bb{:?} stmt{:?})", first.message, first.bb.map(|b| b.0), first.statement_index), format!("{at}{loc}"));
                    }
                    if passes {
                        // mir_const_qualif + mir_promoted (S5): const checking, promotion, simplify.
                        let mut promoted = mir.clone();
                        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::mir::transform::mir_promoted(prog, tcx, env, &provider, &mut promoted))) {
                            Ok(Ok(pre)) => {
                                promoted_ok += 1;
                                n_promoted += promoted.promoted.len();
                                for e in &pre.const_errors {
                                    const_errors.note(format!("{}: {}", e.code, e.message.lines().next().unwrap_or("")), format!("{at}{loc}"));
                                }
                                if let Some(first) = crate::mir::validate::validate(&promoted, tcx).first() {
                                    promoted_invalid += 1;
                                    invalid.note(format!("after mir_promoted: {}", first.message), format!("{at}{loc}"));
                                }
                            }
                            Ok(Err(why)) => blocked.note(why.0.to_string(), format!("{at}{loc}")),
                            Err(p) => {
                                if crate::preempt::is_cancelled(&*p) {
                                    std::panic::resume_unwind(p);
                                }
                                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
                                panicked.note(format!("mir_promoted: {}", msg.split(": ").next().unwrap_or("")), format!("{at}{loc} [{msg}]"));
                                panics += 1;
                            }
                        }
                    }
                }
                Ok(Err(err)) => {
                    let line = line_of(prog, err.span.file, err.span.lo);
                    unbuilt.note(reason_key(&err.reason), format!("{at}{loc} @ {}:{line}", prog.file_paths[err.span.file as usize]))
                }
                Err(p) => {
                    if crate::preempt::is_cancelled(&*p) {
                        std::panic::resume_unwind(p);
                    }
                    panics += 1;
                    let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "panic".into());
                    let (key, detail) = match msg.split_once(": ") { Some((k, v)) => (k.to_string(), format!(" [{v}]")), None => (msg.clone(), String::new()) };
                    panicked.note(key, format!("{at}{loc}{detail}"));
                }
            }
        }
    }
    eprintln!(
        "makepad-run mir-build: {owners} owners ({typeck_fail} failed typeck), {built} bodies built, {validate_fail} failed validation, {panics} panicked, {} not built; cpu {:.1} ms",
        unbuilt.counts.values().sum::<usize>(),
        (relax_backend::cpu_ns() - t0) as f64 / 1e6
    );
    if passes {
        eprintln!("makepad-run mir-build: mir_promoted: {promoted_ok} bodies ok ({promoted_invalid} invalid afterwards), {n_promoted} promoted bodies, {} blocked, {} const-check errors", blocked.counts.values().sum::<usize>(), const_errors.counts.values().sum::<usize>());
    }
    for (kind, (all, ok)) in &by_kind {
        eprintln!("makepad-run mir-build: {kind:>12}: {ok}/{all} built");
    }
    for (title, h) in [("not built", &unbuilt), ("INVALID", &invalid), ("PANIC", &panicked), ("BLOCKED", &blocked), ("CONST ERROR", &const_errors)] {
        let mut rows: Vec<_> = h.counts.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1));
        for (reason, n) in rows {
            eprintln!("makepad-run mir-build: {title} {n:>6}  {reason}   e.g. {}", h.example.get(reason).map_or("", |s| s.as_str()));
        }
    }
    validate_fail + panics
}
