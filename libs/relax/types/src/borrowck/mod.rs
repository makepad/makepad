//! Relax's borrow checker (plans/relax-borrowck.md): rustc's move / initialization / borrow
//! rules on a MIR-like IR (BIR) built from typeck::Body, run lazily.

pub mod driver;
pub mod borrowck_errors;
pub mod check;
pub mod diagnostics;
pub mod field_views;
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
pub mod lazy;
pub mod event;
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
    /// the field borrows (MIR spans and places) the export to rustc rewrites (field_views::edit)
    pub fv_records: Vec<field_views::FvRecordRaw>,
    /// conflicts accepted by Polonius / disjoint indices, with their export facts (field_views::edit)
    pub relaxed_records: Vec<field_views::RelaxedRecord>,
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

/// `check_body` with the callee summaries of a caller's summary context. The one entry every
/// caller reaches: see driver.rs.
pub fn check_body_in(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &Body, fv_cx: Option<&std::rc::Rc<field_views::SummaryCx>>) -> Verdict {
    crate::preempt::point();
    let _cost_borrowck = relax_codegen::fncost::phase(2);
    let checkpoint = tcx.dcx.checkpoint();
    let mut result = driver::check_root(cx, prog, tcx, body, fv_cx);
    if !result.errors.is_empty() || !body.errors.is_empty() { tcx.dcx.note_reported_error(); }
    let (errors, warnings) = tcx.dcx.take_since(checkpoint);
    for error in errors {
        let owner = if error.cause.body_id == crate::program::NO_DEF { body.def } else { error.cause.body_id };
        let mut diagnostic = diagnostics::Diag::new("", crate::mir::Span { file: prog.def(owner).file, lo: error.cause.span, hi: error.cause.span }, error.message);
        diagnostic.labels.push((diagnostic.span, String::new(), true));
        result.errors.push(diagnostics::capture(prog, owner, &diagnostic));
    }
    for warning in warnings { eprintln!("warning: {}", warning.message); }
    result
}

/// An anonymous constant that is a body owner of its own (typeck-audit 1.27) is const-checked
/// alone: rustc_const_eval check_consts on its MIR.
pub fn check_anon_const(cx: &crate::tcx::EvalCx<'_>, prog: &Program, tcx: &Tcx, body: &Body, expr: crate::ast::ExprId) -> Vec<event::BorrowError> {
    driver::check_anon_const(cx, prog, tcx, body, expr)
}

/// MAKEPAD_RUN_BORROWCK=moves: only the move / initialization checks (no regions).
pub fn regions_enabled(options: &crate::options::Options) -> bool {
    options.borrowck != crate::options::Borrowck::Moves
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
    let t0 = relax_backend::cpu_ns();
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
        let c0 = relax_backend::cpu_ns();
        let v = check_body(&crate::tcx::EvalCx::new(prog, &tcx, crate::tcx::TypingEnv::fully_monomorphized(&prog), &crate::tcx::EvalState::new()), prog, tcx, &body);
        note_verdict(&v);
        bck_ns += relax_backend::cpu_ns() - c0;
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
        eprintln!("makepad-run borrowck: {} owners ({} failed typeck polymorphically), {} bodies, {} errors; borrowck cpu {:.1} ms, total {:.1} ms", n_fns, n_typeck_fail, n_bodies, errors, bck_ns as f64 / 1e6, (relax_backend::cpu_ns() - t0) as f64 / 1e6);
        for (k, n) in &unchecked {
            eprintln!("makepad-run borrowck: unchecked {:>6}  {}   e.g. {}", n, k, unchecked_ex.get(k).map_or("", |s| s.as_str()));
        }
    }
    finish_check(prog);
    errors
}

static FV_ONLY: relax_session::sync::Mutex<Vec<event::BorrowError>> = relax_session::sync::Mutex::new(Vec::new());
static FV_RECORDS: relax_session::sync::Mutex<Vec<field_views::FvRecordRaw>> = relax_session::sync::Mutex::new(Vec::new());
static RELAXED_RECORDS: relax_session::sync::Mutex<Vec<field_views::RelaxedRecord>> = relax_session::sync::Mutex::new(Vec::new());

/// `makepad check`: remember a verdict's field-view-only acceptances (relaxed crates).
pub fn note_verdict(v: &Verdict) {
    FV_ONLY.lock().unwrap_or_else(|e| e.into_inner()).extend(v.fv_only.iter().cloned());
    FV_RECORDS.lock().unwrap_or_else(|e| e.into_inner()).extend(v.fv_records.iter().cloned());
    RELAXED_RECORDS.lock().unwrap_or_else(|e| e.into_inner()).extend(v.relaxed_records.iter().cloned());
}

/// End of `makepad check`: the "accepted only with field views" report (what rustc would reject)
/// and, with MAKEPAD_RUN_FV_EXPORT=<dir>, the rewritten sources for rustc.
pub fn finish_check(prog: &Program) {
    let fvo = std::mem::take(&mut *FV_ONLY.lock().unwrap_or_else(|e| e.into_inner()));
    let recs = std::mem::take(&mut *FV_RECORDS.lock().unwrap_or_else(|e| e.into_inner()));
    // Polonius / disjoint-index acceptances: one relaxed note each (`--strict-report`), with the
    // structured edits `makepad fix --to-strict` applies when a rewrite is known.
    let relaxed = std::mem::take(&mut *RELAXED_RECORDS.lock().unwrap_or_else(|e| e.into_inner()));
    for n in field_views::edit::plan_relaxed(prog, &relaxed) { crate::relaxed::note(n); }
    if !fvo.is_empty() {
        eprintln!("makepad check: {} conflict(s) accepted only in relaxed Rust (field views, extended two-phase borrows, Polonius, disjoint indices; rustc would reject them):", fvo.len());
        for d in &fvo {
            eprintln!("{}", d.render());
        }
    }
    if let Ok(dir) = std::env::var("MAKEPAD_RUN_FV_EXPORT") {
        for r in field_views::edit::plan(prog, &recs) {
            let path = &prog.file_paths[r.file as usize];
            let rel = path.trim_start_matches('/');
            let out = std::path::Path::new(&dir).join(rel);
            if let Some(p) = out.parent() {
                let _ = std::fs::create_dir_all(p);
            }
            let pf = &prog.files[r.file as usize];
            let _ = std::fs::write(&out, crate::relaxed::apply(&pf.src[..pf.src_len as usize], &r.edits).0);
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
pub fn summaries_of(prog: &Program, tcx: &Tcx, insts: &[(crate::program::DefId, Vec<crate::types::TyId>)]) -> Vec<Vec<field_views::Summary>> {
    summaries_with(prog, tcx, insts, None)
}

/// `summaries_of` in a caller's summary context (the live edit session's, `field_views::SummaryCx`).
pub fn summaries_with(prog: &Program, tcx: &Tcx, insts: &[(crate::program::DefId, Vec<crate::types::TyId>)], fv_cx: Option<&std::rc::Rc<field_views::SummaryCx>>) -> Vec<Vec<field_views::Summary>> {
    field_views::summaries_with(prog, tcx, &driver::MirBodies::new(prog, tcx), insts, fv_cx)
}

/// "mut {self.n}, ref {self.items}" for a summary of param `k` of `d`.
pub fn describe_summary(prog: &Program, tcx: &Tcx, d: crate::program::DefId, k: usize, s: &field_views::Summary) -> String {
    field_views::describe_summary(prog, tcx, d, k, s)
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
#[cfg(test)]
mod root_cx_tests;
#[cfg(test)]
mod mir_source_tests;
