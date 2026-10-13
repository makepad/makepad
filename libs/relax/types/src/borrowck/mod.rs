//! Relax's borrow checker (plans/relax-borrowck.md): rustc's move / initialization / borrow
//! rules on a MIR-like IR (BIR) built from typeck::Body, run lazily.

pub mod bir;
pub mod borrowck_errors;
pub mod check;
pub mod mir_nll;
pub mod polonius;
pub mod root_cx;
#[cfg(test)]
mod check_tests;
mod used_muts;
pub use check::mir_borrowck;
pub mod borrow_set;
pub mod dataflow;
#[cfg(test)]
mod dataflow_tests;
pub mod def_use;
pub mod path_utils;
pub mod place_ext;
pub mod places_conflict;
pub mod prefixes;
#[cfg(test)]
mod places_tests;
pub mod body;
pub mod build;
pub mod capture;
pub mod diag;
pub mod export;
pub mod fv;
pub mod lazy;
pub mod event;
pub mod moves;
pub mod nll;
pub mod pat;
pub mod promote;
pub mod qualifs;
pub mod rty;
pub mod tyx;
pub mod universal_regions;
pub mod region_infer;
pub mod constraints;
pub mod handle_placeholders;
pub mod implied_bounds;
pub mod type_check;
pub mod infer_ctxt;
pub mod renumber;

use crate::program::Program;
use crate::tcx::Tcx;
use crate::typeck::Body;

pub struct Verdict {
    /// Structured diagnostics shared by checker consumers.
    pub errors: Vec<event::BorrowError>,
    /// bodies (fn + closures) the builder could not express, with why
    pub unchecked: Vec<String>,
    /// relaxed crates: conflicts accepted only with field views (rustc would reject them)
    pub fv_only: Vec<event::BorrowError>,
    /// (file, span of the field borrow, its place text, Default) for the export rewrite
    pub fv_records: Vec<export::FvRecord>,
    pub bodies: usize,
}

/// Mode from MAKEPAD_RUN_BORROWCK: "off" disables the checker.
pub fn enabled(options: &crate::options::Options) -> bool {
    options.borrowck != crate::options::Borrowck::Off
}

/// The identity args of a generic fn (its own params): a polymorphic check.
pub fn identity_args(tcx: &Tcx, prog: &Program, def: crate::program::DefId) -> Option<Vec<crate::types::TyId>> {
    Some(tcx.fn_sig(prog, def)?.generic_indices.identity_types_consts(&tcx.tys))
}

/// Completed checks release their body-owned caches before acknowledging settlement.
pub fn settle() -> std::sync::mpsc::Receiver<u64> {
    lazy::settle()
}

/// Each body's environment owns its dependent type-query caches.
pub fn check_body(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &Body) -> Verdict {
    check_body_in(cx, prog, tcx, body, None)
}

/// `check_body` with the callee summaries of a caller's summary context.
pub fn check_body_in(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &Body, fv_cx: Option<&std::rc::Rc<fv::SummaryCx>>) -> Verdict {
    crate::preempt::point();
    let _cost_borrowck = crate::fncost::phase(2);
    let env = crate::tcx::TypingEnv { limit: crate::tcx::RecursionLimit::for_item(prog, body.def), param_env: body.param_env.unwrap_or_default(), mode: crate::tcx::TypingMode::borrowck(tcx, prog, body.def) };
    let checkpoint = tcx.dcx.checkpoint();
    let mut tx = tyx::Tx::new(prog, tcx, env, cx.state);
    if let Some(fv_cx) = fv_cx { tx.fv = fv_cx.clone(); }
    let rty = rty::Rty::new(&tx);
    let mut result = check_body_with(prog, tcx, &tx, &rty, body);
    if !result.errors.is_empty() || !body.errors.is_empty() { tx.errors.note_reported_error(); }
    let (errors, warnings) = tx.errors.take_since(checkpoint);
    for error in errors {
        let owner = if error.cause.body_id == crate::program::NO_DEF { body.def } else { error.cause.body_id };
        let diagnostic = diag::Diag::new("", error.message, bir::Span { lo: error.cause.span, hi: error.cause.span });
        result.errors.push(diag::to_error(prog, prog.def(owner).file, owner, &diagnostic));
    }
    for warning in warnings { eprintln!("warning: {}", warning.message); }
    result
}

fn check_body_with(prog: &Program, tcx: &Tcx, tx: &tyx::Tx, rty: &rty::Rty, body: &Body) -> Verdict {
    let options = &prog.session.clone().options;
    tx.obligation_owner.set(body.def);
    if body.args.iter().any(|a| matches!(tcx.tys.kind(*a), crate::types::TyKind::Param(_))) {
        tx.set_generic_env(body.def, body.args.len());
    } else {
        tx.reset_env();
    }
    let tx = &*tx;
    *rty.closure_signatures.borrow_mut() = body.closure_sigs.clone();
    let name = prog.def_path(body.def);
    let mut b = build::Build::new(tx, body, name);
    let mut v = Verdict { errors: Vec::new(), unchecked: Vec::new(), fv_only: Vec::new(), fv_records: Vec::new(), bodies: 0 };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        b.build_body();
        let closures = std::mem::take(&mut b.closures_out);
        let main = std::mem::replace(&mut b.out, empty());
        let mut all = vec![main];
        for c in closures {
            all.push(c.bir);
        }
        for child in body::const_children(prog, tcx, body) {
            let mut sub = build::Build::new(tx, body, format!("{}::{:?}", prog.def_path(body.def), child.kind));
            sub.build_const_child(child);
            all.push(sub.out);
            all.extend(sub.closures_out.into_iter().map(|c| c.bir));
        }
        all
    }));
    let mut all = match r {
        Ok(a) => a,
        Err(p) => {
            if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
            v.unchecked.push(format!("{}: internal error in the borrowck builder", prog.def_path(body.def)));
            return v;
        }
    };
    for parent in &all {
        for child in &parent.closures { rty.closure_parents.borrow_mut().insert(*child, parent.closure_expr); }
    }
    let dump = std::env::var("MAKEPAD_RUN_BIR_DUMP").ok();
    // closure types' skeletons: their captures and return types; their Copy-ness
    for bir in &all {
        if bir.closure_kind.is_some() {
            if let Some(ce) = closure_expr_of(&all, bir) {
                let copy = bir.captures.iter().all(|c| match c.kind {
                    0 => tx.is_copy(c.ty),
                    1 => true,
                    _ => false,
                });
                tx.closure_copy.borrow_mut().insert((bir.file, ce), copy);
                tx.reset_copy_memo();
                let caps: Vec<crate::types::TyId> = bir.captures.iter().map(|c| c.ty).collect();
                let ret = bir.locals.get(0).map_or(tx.unit(), |l| l.ty);
                let params = (2..=bir.n_args as usize).filter(|l| *l < bir.locals.len()).filter_map(|l| {
                    let ty = bir.locals[l].ty;
                    let slots = if !bir.expected_sig && !bir.hr_args.contains(&(l as bir::Local)) && bir.infer_params.contains(&(l as bir::Local)) {
                        (0..rty.n_slots(ty)).collect()
                    } else { rty::signature::external_closure_input_slots(rty, ce, l - 2) };
                    (!slots.is_empty()).then_some((l - 2,ty,slots))
                }).collect();
                rty.closures.borrow_mut().insert((bir.file, ce), rty::ClosureSkel { caps, params, ret });
            }
        }
    }
    let query_store = qualifs::Store::default();
    let queries = qualifs::Provider::new(tx, body, &query_store);
    let mut promoteds = Vec::with_capacity(all.len());
    for bir in &mut all {
        crate::preempt::point();
        if bir.unsupported.is_some() { promoteds.push(Vec::new()); continue; }
        let collected = promote::collect_temps_and_candidates(bir);
        match promote::validate_candidates(tx, bir, &queries, collected) {
            Ok(validated) => promoteds.push(promote::promote_candidates(tx, bir, validated)),
            Err(why) => {
                bir.unsupported = Some(format!("promotion query blocked: {}", why.0));
                promoteds.push(Vec::new());
            }
        }
    }
    let regions = regions_enabled(options);
    let relaxed = fv::relaxed(prog, body.def);
    // field views of closure captures (what each closure does through a captured reference)
    let mut closure_fv: crate::fxhash::HashMap<u32, Vec<fv::Summary>> = Default::default();
    for bir in &all {
        if let (Some(ce), true) = (bir.closure_expr, bir.closure_kind.is_some() && bir.unsupported.is_none()) {
            closure_fv.insert(ce, fv::closure_captures(prog, tcx, tx, bir));
        }
    }
    let mut reqs: crate::fxhash::HashMap<u32, Vec<nll::ClosureOutlivesRequirement>> = Default::default();
    // nested closures first (they follow their parent in the list), the fn body last
    let mut order: Vec<usize> = (1..all.len()).rev().collect();
    order.push(0);
    for i in order {
        crate::preempt::point();
        let bir = &all[i];
        v.bodies += 1;
        if let Some(why) = &bir.unsupported {
            v.unchecked.push(format!("{}: {}", bir.name, why));
            continue;
        }
        if let Some(f) = &dump {
            if bir.name.contains(f.as_str()) {
                eprintln!("{}", bir_dump(tx, bir));
            }
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let md = moves::MoveData::new(tx, bir);
            let order = moves::rpo(bir);
            let flow = moves::init_flow(&md, bir, &order);
            let mut mc = moves::MoveCheck::new(tx, bir, &md);
            mc.run(&flow, &order);
            if crate::lint::enabled() && mc.diags.is_empty() && bir.closure_kind.is_none() && i == 0 {
                // rustc gather_used_muts: a `mut` local no reachable code assigns (declared in
                // dead code, or never initialized) counts as used
                let mut used = mc.used_mut.clone();
                let mut assigned = moves::bits_new(bir.locals.len());
                for (b, blk) in bir.blocks.iter().enumerate() {
                    if flow.entry.get(b).map_or(true, |e| e.is_none()) { continue; }
                    for st in &blk.stmts { if let bir::StmtKind::Assign(p, _) = &st.kind { moves::bit_set(&mut assigned, p.local as usize); } }
                    if let Some(bir::Term { kind: bir::TermKind::Call { dest, .. }, .. }) = &blk.term { moves::bit_set(&mut assigned, dest.local as usize); }
                }
                for l in 0..bir.locals.len() { if !moves::bit_get(&assigned, l) && bir.locals[l].kind == bir::LocalKind::User { moves::bit_set(&mut used, l); } }
                lint_unused_mut(prog, body.def, bir, &used);
            }
            let mut diags = mc.diags;
            for promoted in &promoteds[i] {
                let md = moves::MoveData::new(tx, promoted);
                let mut mc = moves::MoveCheck::new(tx, promoted, &md);
                mc.check_promoted_moves();
                diags.extend(mc.diags);
            }
            let mut req = Vec::new();
            let mut fvo = Vec::new();
            let mut fvr = Vec::new();
            if regions {
                let o = nll::run_with_promoted(tx, rty, bir, &promoteds[i], &reqs, &md, relaxed, &closure_fv, prog, tcx);
                diags.extend(o.diags);
                req = o.requirements;
                fvo = o.fv_only;
                fvr = o.fv_records;
            }
            (diags, req, fvo, fvr)
        }));
        match r {
            Ok((diags, req, fvo, fvr)) => {
                for (sp, t, dflt, access, hoist) in fvr {
                    v.fv_records.push(export::FvRecord { file: bir.file, span: sp, place: t, default: dflt, access, hoist });
                }
                for d in fvo {
                    v.fv_only.push(diag::to_error(prog, bir.file, body.def, &d));
                }
                if let Some(ce) = closure_expr_of(&all, bir) {
                    if bir.closure_kind.is_some() {
                        reqs.insert(ce, req);
                    }
                }
                for d in diags {
                    v.errors.push(diag::to_error(prog, bir.file, body.def, &d));
                }
            }
            Err(p) => {
                if crate::preempt::is_cancelled(&*p) { std::panic::resume_unwind(p); }
                v.unchecked.push(format!("{}: internal error in the borrow checker", bir.name));
            }
        }
    }
    for (goal, owner) in tx.pending_traits.borrow().iter() {
        v.unchecked.push(format!("{}: unresolved trait property {:?}", prog.def_path(*owner), goal.predicate));
    }
    v
}

/// MAKEPAD_RUN_BORROWCK=moves: only the move / initialization checks (no regions).
pub fn regions_enabled(options: &crate::options::Options) -> bool {
    options.borrowck != crate::options::Borrowck::Moves
}

fn closure_expr_of(all: &[bir::Bir], b: &bir::Bir) -> Option<u32> {
    // the closure expr id: recorded by the builder in its name suffix table
    let _ = all;
    b.closure_expr
}

pub(super) fn empty() -> bir::Bir {
    bir::Bir {
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

/// Text form of a BIR (MAKEPAD_RUN_BIR_DUMP=<name substring>).
pub fn bir_dump(tx: &tyx::Tx, bir: &bir::Bir) -> String {
    let mut s = format!("bir {} ({} args)\n", bir.name, bir.n_args);
    for (i, l) in bir.locals.iter().enumerate() {
        s.push_str(&format!("  let _{}: {}{}{}\n", i, if l.mutable { "mut " } else { "" }, tx.ty_str(l.ty), l.name.as_ref().map_or(String::new(), |n| format!("  // {}", n))));
    }
    for (bi, b) in bir.blocks.iter().enumerate() {
        s.push_str(&format!("  bb{}:\n", bi));
        for st in &b.stmts {
            s.push_str(&format!("    {:?}\n", st.kind));
        }
        match &b.term {
            Some(t) => s.push_str(&format!("    -> {:?}\n", t.kind)),
            None => s.push_str("    -> (none)\n"),
        }
    }
    s
}

/// `MAKEPAD_RUN_BORROWCK_ALL=1 makepad check <pkg>`: type-check and borrow-check every fn with a body
/// in every crate of the graph except Relax's std (generic ones polymorphically); prints the
/// diagnostics and a summary. The makepad false-positive gate.
pub fn check_all(prog: &Program, tcx: &Tcx, with_std: bool) -> usize {
    let options = &prog.session.clone().options;
    let skip_generic = options.borrowck_all == crate::options::BorrowckAll::Nongeneric;
    use crate::program::DefKind;
    let std_crates: Vec<u32> = ["core", "alloc", "std"].iter().filter_map(|n| prog.crates.iter().position(|c| prog.syms.str(c.name) == *n).map(|i| i as u32)).collect();
    let mut n_fns = 0usize;
    let mut n_typeck_fail = 0usize;
    let mut n_bodies = 0usize;
    let mut errors = 0usize;
    let mut unchecked: std::collections::BTreeMap<String, usize> = Default::default();
    let mut unchecked_ex: std::collections::BTreeMap<String, String> = Default::default();
    let t0 = crate::clock::cpu_ns();
    let mut bck_ns = 0u64;
    let only = options.borrowck_only.clone();
    for owner in body::owners(prog) {
        let def = owner.owner;
        let d = prog.def(def);
        if !with_std && std_crates.contains(&d.krate) { continue; }
        if let Some(o) = &only {
            if !prog.def_path(def).contains(o.as_str()) {
                continue;
            }
        }
        n_fns += 1;
        let args = owner.identity_args(prog, tcx);
        if skip_generic && (!args.is_empty() || prog.def(d.parent).kind == DefKind::Trait) {
            continue;
        }
        // rustc_borrowck/src/lib.rs:115 and root_cx.rs: bodies are checked with
        // identity arguments and their analysis predicates, before codegen's
        // instantiate_mir_and_normalize_erasing_regions.
        let body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tcx.typeck(prog, def)));
        let body = match body {
            Ok(b) if b.errors.is_empty() => b,
            r => {
                if std::env::var_os("MAKEPAD_RUN_BORROWCK_TYFAIL").is_some() {
                    match &r {
                        Ok(b) => eprintln!("makepad-run borrowck: typeck failed polymorphically: {}: {}", prog.def_path(def), b.errors.first().map(|e| e.as_str()).unwrap_or("")),
                        Err(_) => eprintln!("makepad-run borrowck: typeck panicked polymorphically: {}", prog.def_path(def)),
                    }
                }
                n_typeck_fail += 1;
                continue;
            }
        };
        let c0 = crate::clock::cpu_ns();
        let v = check_body(&crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &crate::tcx::EvalState::new()), prog, tcx, &body);
        note_verdict(&v);
        bck_ns += crate::clock::cpu_ns() - c0;
        n_bodies += v.bodies;
        for u in &v.unchecked {
            let why = u.rsplit(": ").next().unwrap_or("").to_string();
            *unchecked.entry(why.clone()).or_insert(0) += 1;
            unchecked_ex.entry(why).or_insert_with(|| u.clone());
        }
        for e in &v.errors {
            println!("{}", e.render());
        }
        errors += v.errors.len();
    }
    if options.verbose {
        eprintln!("makepad-run borrowck: {} owners ({} failed typeck polymorphically), {} bodies, {} errors; borrowck cpu {:.1} ms, total {:.1} ms", n_fns, n_typeck_fail, n_bodies, errors, bck_ns as f64 / 1e6, (crate::clock::cpu_ns() - t0) as f64 / 1e6);
        let ph: Vec<String> = nll::PHASE_NS.iter().map(|x| format!("{:.0}", x.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6)).collect();
        eprintln!("makepad-run borrowck: nll phases ms (constraints, liveness, solve, loans, regions): {}", ph.join(" "));
        for (k, n) in &unchecked {
            eprintln!("makepad-run borrowck: unchecked {:>6}  {}   e.g. {}", n, k, unchecked_ex.get(k).map_or("", |s| s.as_str()));
        }
    }
    finish_check(prog);
    errors
}

static FV_ONLY: relax_session::sync::Mutex<Vec<event::BorrowError>> = relax_session::sync::Mutex::new(Vec::new());
static FV_RECORDS: relax_session::sync::Mutex<Vec<export::FvRecord>> = relax_session::sync::Mutex::new(Vec::new());

/// `makepad check`: remember a verdict's field-view-only acceptances (relaxed crates).
pub fn note_verdict(v: &Verdict) {
    FV_ONLY.lock().unwrap_or_else(|e| e.into_inner()).extend(v.fv_only.iter().cloned());
    FV_RECORDS.lock().unwrap_or_else(|e| e.into_inner()).extend(v.fv_records.iter().cloned());
}

/// End of `makepad check`: the "accepted only with field views" report (what rustc would reject)
/// and, with MAKEPAD_RUN_FV_EXPORT=<dir>, the rewritten sources for rustc.
pub fn finish_check(prog: &Program) {
    let fvo = std::mem::take(&mut *FV_ONLY.lock().unwrap_or_else(|e| e.into_inner()));
    let recs = std::mem::take(&mut *FV_RECORDS.lock().unwrap_or_else(|e| e.into_inner()));
    if !fvo.is_empty() {
        eprintln!("makepad check: {} conflict(s) accepted only in relaxed Rust (field views, extended two-phase borrows; rustc would reject them):", fvo.len());
        for d in &fvo {
            eprintln!("{}", d.render());
        }
    }
    if let Ok(dir) = std::env::var("MAKEPAD_RUN_FV_EXPORT") {
        for r in export::rewrite(prog, &recs) {
            let path = &prog.file_paths[r.file as usize];
            let rel = path.trim_start_matches('/');
            let out = std::path::Path::new(&dir).join(rel);
            if let Some(p) = out.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let _ = std::fs::write(&out, &r.new_src);
            for a in &r.applied {
                eprintln!("makepad-run fv-export: {}", a);
            }
            for s in &r.skipped {
                eprintln!("makepad-run fv-export: skipped {}", s);
            }
            eprintln!("makepad-run fv-export: wrote {}", out.display());
        }
    }
}

/// Field-view summaries of fn instances (for live patches: before / after an edit).
pub fn summaries_of(prog: &Program, tcx: &Tcx, insts: &[(crate::program::DefId, Vec<crate::types::TyId>)]) -> Vec<Vec<fv::Summary>> {
    summaries_with(prog, tcx, insts, None)
}

/// `summaries_of` in a caller's summary context (the live edit session's, `fv::SummaryCx`).
pub fn summaries_with(prog: &Program, tcx: &Tcx, insts: &[(crate::program::DefId, Vec<crate::types::TyId>)], fv_cx: Option<&std::rc::Rc<fv::SummaryCx>>) -> Vec<Vec<fv::Summary>> {
    let state = crate::tcx::EvalState::new();
    let mut tx = tyx::Tx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &state);
    if let Some(fv_cx) = fv_cx { tx.fv = fv_cx.clone(); }
    let tx = &tx;
    insts.iter().map(|(d, a)| fv::summaries(prog, tcx, tx, *d, a, 0).as_ref().clone()).collect()
}

/// "mut {self.n}, ref {self.items}" for a summary of param `k` of `d`.
pub fn describe_summary(prog: &Program, tcx: &Tcx, d: crate::program::DefId, k: usize, s: &fv::Summary) -> String {
    let pname = if k == 0 && tcx.fn_sig(prog, d).map_or(false, |x| x.self_kind != 0) { "self".to_string() } else { format!("arg{}", k) };
    let pt = tcx.fn_sig(prog, d).and_then(|x| x.params.get(k).copied());
    let state = crate::tcx::EvalState::new();
    let tx = &tyx::Tx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &state);
    {
        let t = match pt.map(|p| tx.kind(p)) {
            Some(crate::types::TyKind::Ref(_, i, _)) => i,
            _ => return "everything".to_string(),
        };
        match s {
            fv::Summary::Whole => format!("all of `*{}`", pname),
            fv::Summary::Parts(v) => {
                let mut parts = Vec::new();
                for (p, m) in v {
                    parts.push(format!("{}`{}.{}`", if *m { "mutably " } else { "" }, pname, fv::path_str(tx, t, p)));
                }
                if parts.is_empty() {
                    "nothing".to_string()
                } else {
                    parts.join(", ")
                }
            }
        }
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    #[test]
    fn settle_acknowledges_without_a_background_worker() {
        assert_eq!(settle().recv().unwrap(), 0);
    }
}

#[cfg(test)]
mod implied_bounds_tests;
#[cfg(test)]
mod type_check_tests;

/// rustc_borrowck lib.rs lint_unused_mut: a user binding (or argument) declared `mut` that no
/// write or mutable borrow needed -- `unused_mut`. Bindings named `_..` are left out, and so is
/// a binding whose declaration text is not `mut <name>` (macro expansions).
fn lint_unused_mut(prog: &Program, owner: crate::program::DefId, bir: &bir::Bir, used: &moves::Bits) {
    let Some(f) = prog.files.get(bir.file as usize) else { return };
    let src = &f.src;
    for (i, ld) in bir.locals.iter().enumerate() {
        if !ld.mutable || moves::bit_get(used, i) || ld.is_ref_binding { continue; }
        if !matches!(ld.kind, bir::LocalKind::User | bir::LocalKind::Arg) { continue; }
        let Some(name) = ld.name.as_deref() else { continue };
        if name.starts_with('_') { continue; }
        let (lo, hi) = (ld.span.lo as usize, (ld.span.hi as usize).min(src.len()));
        if lo >= hi { continue; }
        let word = |j: usize| j + name.len() <= src.len() && &src[j..j + name.len()] == name.as_bytes()
            && (j == 0 || !(src[j - 1] == b'_' || src[j - 1].is_ascii_alphanumeric()))
            && (j + name.len() >= src.len() || !(src[j + name.len()] == b'_' || src[j + name.len()].is_ascii_alphanumeric()));
        let Some(pos) = (lo..hi).find(|&j| word(j)) else { continue };
        if pos < 4 || &src[pos - 4..pos] != b"mut " { continue; }
        crate::lint::emit(prog, owner, &crate::lint::UNUSED_MUT, bir.file, (pos - 4) as u32,
            &format!("variable does not need to be mutable -- help: remove this `mut` (`{}`)", name));
    }
}
