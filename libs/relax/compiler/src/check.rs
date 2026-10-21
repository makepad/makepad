//! `makepad check`: the app check, like `cargo check`: every function of the package's workspace
//! crates (every loaded crate whose sources are in the workspace, std excepted) is type- and
//! borrow-checked (generic ones polymorphically, as rustc checks generic bodies once); nothing
//! is compiled or run. Errors print as one line each; the exit code is 0 when there are none.
//!
//! `makepad selftest`: a test of Relax itself on a package -- the same checks, then every
//! non-generic function is compiled to machine code by the run's own per-function compile
//! (`jit::compile_slot`), and so is every instance, closure and glue those reach (rustc's
//! monomorphization collector for `build-pass`), on `MAKEPAD_RUN_JOBS` threads. Every failure prints
//! as one line (`file:line:col: error|internal: ..`); `internal` marks a makepad run bug (a compiler
//! panic, an error without a user-facing code, or an error inside std or a dependency rustc
//! builds). `--root-only` and the watch session's deep check (`collect_deep`) cover the root
//! crate (and what it reaches) only.

macro_rules! eprintln {
    ($($arg:tt)*) => { if !crate::diag::enabled() { std::eprintln!($($arg)*); } };
}

use relax_types::next_solver::{Goal, ParamEnvId};
use crate::jit::{self, FnKey, Unit};
use crate::program::DefKind;

/// Errors of earlier phases (`file:line:col: ..` lines) that belong to the root crate's files.
pub fn own_errors(prog: &crate::program::Program, root: u32, errs: &[String], phase: &'static str) -> Vec<String> {
    let mut paths: Vec<&str> = Vec::new();
    for (i, p) in prog.file_paths.iter().enumerate() {
        if prog.file_crate.get(i).copied() == Some(root) {
            paths.push(p.as_str());
        }
    }
    errs.iter().filter(|e| paths.iter().any(|p| e.starts_with(p))).map(|e| {
        if crate::diag::enabled() { crate::diag::record(e.clone(),crate::diag::from_legacy(e,phase)) }
        else { e.clone() }
    }).collect()
}

/// The crates a one-shot `makepad check` covers: the root and every loaded crate whose root file
/// is in the workspace, except std (MAKEPAD_RUN_STD may live inside it) and proc-macro crates (their
/// code runs in the macro host, never in the program).
pub fn workspace_crates(prog: &crate::program::Program, root: u32) -> Vec<u32> {
    fn canon(p: &str) -> String {
        std::fs::canonicalize(p).map(|c| c.display().to_string()).unwrap_or_else(|_| p.to_string())
    }
    let ws = format!("{}/", canon(&crate::run::workspace_dir()).trim_end_matches('/'));
    let std_dir = format!("{}/", canon(&crate::std_dir()).trim_end_matches('/'));
    let mut out = Vec::new();
    for (k, c) in prog.crates.iter().enumerate() {
        let path = canon(c.root_path());
        let inside = path.starts_with(&ws) && !path.starts_with(&std_dir);
        if k as u32 == root || (inside && !c.proc_macro && !matches!(prog.syms.str(c.name), "core" | "alloc" | "std" | "proc_macro")) {
            out.push(k as u32);
        }
    }
    out
}

/// The crates `makepad check` checks: the application's own -- the root and the crates of the
/// app's project tree that are not the framework (the one app-crate rule the check server and
/// Gandalf use, reload.rs gandalf_app_roots), as `cargo check` re-checks only what changed.
pub fn app_crates(prog: &crate::program::Program, root: u32) -> Vec<u32> {
    (0..prog.crates.len() as u32).filter(|&k| k == root || crate::borrowck::event::app_crate(prog, k)).collect()
}

/// `f` over `items` on MAKEPAD_RUN_JOBS threads (at most 16), results in `items` order.
fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let jobs = std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(16));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out: relax_session::sync::Mutex<Vec<(usize, R)>> = relax_session::sync::Mutex::new(Vec::with_capacity(items.len()));
    std::thread::scope(|sc| {
        for w in 0..jobs.min(items.len().max(1)) {
            let (next, out, f) = (&next, &out, &f);
            std::thread::Builder::new().name(format!("run-check-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(x) = items.get(i) else { break };
                    local.push((i, f(x)));
                }
                out.lock().unwrap().extend(local);
            }).expect("check worker");
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|(i, _)| *i);
    v.into_iter().map(|(_, r)| r).collect()
}

/// `selftest`: every workspace crate, then every function compiled (see the module doc); else
/// the app check of the application's crates.
pub fn check_crate(u: &mut Unit, root: u32, errors: Vec<String>, root_only: bool, selftest: bool, workspace: bool) -> i32 {
    let crates = if root_only { vec![root] } else if selftest || workspace { workspace_crates(&u.prog, root) } else { app_crates(&u.prog, root) };
    check_crate_impl(u, root, &crates, errors, true, selftest).0
}

/// The deep check's diagnostics without printing a second stream into a watch response.
pub fn collect_deep(u: &mut Unit, root: u32) -> Vec<crate::diag::Diagnostic> {
    check_crate_impl(u, root, &[root], Vec::new(), false, true).1
}

fn check_crate_impl(u: &mut Unit, root: u32, crates: &[u32], mut errors: Vec<String>, emit: bool, compile: bool) -> (i32, Vec<crate::diag::Diagnostic>) {
    let t_check = std::time::Instant::now();
    // a constant that cannot be evaluated is one reported failure, not the end of the check
    jit::COMPILE_ERRORS_FATAL.store(false, std::sync::atomic::Ordering::Relaxed);
    // lints (rustc's deny-by-default ones are errors; warn-level ones are printed)
    relax_types::lint::enable();
    crate::layout::set_root_crate(root);
    let mut own = vec![false; u.prog.crates.len()];
    for &k in crates {
        own[k as usize] = true;
        u.prog.crates[k as usize].validate = true;
    }
    u.prog.own_crates = own.clone();
    errors.extend(u.prog.strict_import_errors(&own));
    errors.extend(relax_types::lint::import_resolution_diags(&mut u.prog, &own));
    // every checked definition's signature, on MAKEPAD_RUN_JOBS threads (queries over the shared
    // context); errors in definition order
    {
        let defs: Vec<crate::program::DefId> = (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).collect();
        let (prog, tcx) = (&u.prog, &u.tcx);
        for e in par_map(&defs, |&d| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut e = tcx.check_def_signature(prog, d);
                if prog.def(d).kind == DefKind::Impl { e.extend(relax_types::unsafety::check_impl(prog, tcx, d)); }
                e
            }))
            .unwrap_or_else(|_| vec![format!("internal compiler error while checking the signature of {} -- makepad run bug, report it", prog.def_path(d))])) {
            errors.extend(e);
        }
    }
    errors.extend(relax_types::item_checks::duplicate_items(&u.prog, &own));
    errors.extend(relax_types::item_checks::duplicate_exported_macros(&u.prog, &own));
    errors.extend(relax_types::item_checks::super_paths(&u.prog, &own));
    errors.extend(relax_types::item_checks::impl_trait_path_privacy(&u.prog, &u.tcx, &own));
    errors.extend(relax_types::item_checks::private_assoc_types(&u.prog, &u.tcx, &own));
    errors.extend(relax_types::item_checks::gated_syntax(&u.prog, &own));
    errors.extend(relax_types::item_checks::node_attribute_targets(&u.prog, &own));
    errors.extend(relax_types::item_checks::param_attributes(&u.prog, &own));
    errors.extend(relax_types::item_checks::foreign_blocks(&u.prog, &own));
    errors.extend(relax_types::item_checks::duplicate_symbols(&u.prog, &own));
    errors.extend(relax_types::item_checks::global_allocators_and_lang_items(&u.prog, &own));
    if u.prog.crates[root as usize].is_bin { errors.extend(relax_types::item_checks::entry_point(&u.prog, root)); }
    if u.prog.crates[root as usize].is_bin { errors.extend(relax_types::item_checks::entry_symbol_conflict(&u.prog, &own, root)); }
    errors.extend(relax_types::lint_levels::check(&u.prog, &own));
    relax_types::lint_pass::check_items(&u.prog, &u.tcx, &own);
    relax_types::lint_dead::reset();
    errors.extend(u.prog.import_privacy_errors(&own));
    errors.extend(u.prog.attribute_errors(&own));
    errors.extend(u.prog.ast_validation_errors(&own));
    // item well-formedness (rustc wfcheck): after the signatures, and only when they are sound -- an
    // item with an unresolved or ill-counted type has no well-formedness to speak of
    if errors.is_empty() {
        let defs: Vec<crate::program::DefId> = (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).map(|i| crate::program::DefId(i as u32)).collect();
        let (prog, tcx) = (&u.prog, &u.tcx);
        // (a constant a well-formedness check evaluates may find the compiler's owner busy: preempted
        // work is asked again, as the body checks do)
        for e in par_map(&defs, |&d| {
            let mut tries = 0;
            loop {
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::wfcheck::obligations::check_def(prog, tcx, d)));
                match r {
                    Ok(v) => break v,
                    Err(p) if (crate::preempt::is_preempted(&*p) || p.is::<crate::preempt::NeedsLoad>() || p.is::<crate::preempt::OwnerBusy>()) && tries < 100 => { tries += 1; std::thread::yield_now(); }
                    Err(p) => break if let Some(fatal) = p.downcast_ref::<relax_types::infer::diagnostics::FatalError>() { vec![fatal.0.clone()] } else { vec![format!("internal compiler error while checking the well-formedness of {} ({}) -- makepad run bug, report it", prog.def_path(d),
                        p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())] },
                }
            }
        }) {
            errors.extend(e);
        }
        if errors.is_empty() {
            if let Some(main) = crate::run::find_main(&u.prog, root) {
                if relax_types::wfcheck::obligations::is_entry_main(&u.prog, main) { errors.extend(relax_types::wfcheck::obligations::check_main(&u.prog, &u.tcx, main)); }
            }
        }
    }
    let options = &u.prog.session.clone().options;
    crate::borrowck::lazy::disable_gandalf();
    if options.borrowck_all != crate::options::BorrowckAll::Default {
        let n = crate::borrowck::check_all(&u.prog, &u.tcx, options.borrowck_all == crate::options::BorrowckAll::Std);
        return (if n == 0 { 0 } else { 1 }, Vec::new());
    }
    // MIR borrowck pipeline harness (docs: relax_types::mir::build::driver_borrowck).
    if let Some(mode) = std::env::var_os("MAKEPAD_RUN_MIR_BORROWCK_ALL") {
        let errors = relax_types::mir::build::driver_borrowck::borrowck_all(&u.prog, &u.tcx, mode == "std");
        return (if errors == 0 { 0 } else { 1 }, Vec::new());
    }
    // MIR construction harness (docs: relax_types::mir::build::driver): build + validate every body.
    if let Some(mode) = std::env::var_os("MAKEPAD_RUN_MIR_BUILD_ALL") {
        let failures = relax_types::mir::build::driver::build_all(&u.prog, &u.tcx, mode == "std");
        return (if failures == 0 { 0 } else { 1 }, Vec::new());
    }
    let console = crate::gandalf_out::Console::start(emit && crate::diag::enabled(), !emit);
    let mut borrow_diagnostics = Vec::new();
    let up: *mut Unit = u;
    let mut fns = Vec::new();
    // the checked crates' definitions only (the program's other bodies are never looked at)
    for owner in (0..u.prog.defs.len()).filter(|&i| own[u.prog.defs[i].krate as usize]).filter_map(|i| crate::borrowck::body::CheckedBody::item(&u.prog, crate::program::DefId(i as u32))) {
        let d = u.prog.def(owner.owner);
        // the app check needs no seeds: no signature is asked here (each costs a query; Stage's
        // 9,200 took 3.4 s on one thread), the body's own args are used as they are
        if !compile {
            fns.push((owner, true));
            continue;
        }
        let in_trait = u.prog.def(d.parent).kind == DefKind::Trait;
        let generic = u.tcx.fn_sig(&u.prog, owner.owner).map_or(true, |s| s.n_generics > 0);
        // a fn whose where-clauses on concrete types cannot hold (`where &'a mut (): Clone`) is
        // type-checked but never compiled (rustc: impossible predicates are not codegened)
        // (only selftest compiles: the app check needs no such filter)
        let impossible = if generic || in_trait || !compile { false } else {
            let infcx = relax_types::infer::InferCtxt::new(&u.tcx, crate::tcx::TypingMode::PostAnalysis, crate::tcx::TypingEnv::fully_monomorphized(&u.prog).limit);

            u.tcx.clauses_of(&u.prog, owner.owner).into_iter().any(|predicate| {
                !infcx.evaluate_obligation(&u.prog, Goal { param_env: ParamEnvId::EMPTY, predicate }).may_apply()
            })
        };
        // a provided crate's fns are built by rustc into the loader: JIT code reaches its listed
        // ops through their shims, and an unlisted fn is compiled from source only when a body
        // reaches it -- that body's compile covers it (#[relax_api]); compiling every unlisted fn
        // here would report what never runs on the JIT side
        let provided = u.provided.iter().any(|(k, _)| *k == d.krate);
        fns.push((owner, generic || in_trait || impossible || provided));
    }
    let n_own = fns.len();
    let t_owners = t_check.elapsed();
    // typeck-audit 1.27: anonymous constants outside bodies are body owners of their own
    for anon in crates.iter().flat_map(|&k| crate::borrowck::body::signature_anon_consts(&u.prog, &u.tcx, k)).collect::<Vec<_>>() {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let body = u.tcx.typeck(&u.prog, anon);
            let mut errs = body.errors.to_vec();
            if errs.is_empty() {
                relax_types::lint::check_literals(&u.prog, &u.tcx, &body);
                relax_types::lint_pass::check_body(&u.prog, &u.tcx, &body);
            }
            // rustc check_consts: an anonymous constant is a const context of its own
            if errs.is_empty() && crate::borrowck::enabled(options) {
                if let Some((_, relax_front::def_collector::NodeId::AnonConst(expr))) = u.prog.def_id_to_node_id.get(&anon) {
                    let state = crate::tcx::EvalState::new();
                    let cx = crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &state);
                    for e in crate::borrowck::check_anon_const(&cx, &u.prog, &u.tcx, &body, *expr) {
                        errs.push(e.render());
                    }
                }
            }
            errs
        }));
        match r {
            Ok(e) => {
                // rustc evaluates every array length / const argument written in a signature (used
                // or not): one that fails is reported there (E0080)
                if e.is_empty() && u.tcx.generic_indices_of(&u.prog, anon).types_consts.is_empty() && !matches!(u.prog.def(u.prog.def(anon).parent).kind, DefKind::Trait | DefKind::TypeAlias) {
                    let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
                    let before = u.tcx.dcx.err_count();
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.tcx.const_eval_resolve(&u.prog, env, anon, &[])));
                    let _ = before;
                }
                errors.extend(e)
            }
            Err(_) => errors.push(format!("internal compiler error while checking {}", u.prog.def_path(anon))),
        }
    }
    // every body owner type- and borrow-checked on MAKEPAD_RUN_JOBS threads: typeck and the borrow
    // checker are queries over the shared program and type context (as the run's typeck-ahead
    // workers use them); results are merged in owner order
    struct Outcome { errors: Vec<String>, diags: Vec<crate::diag::Diagnostic>, seed: Option<crate::program::DefId>, retry: bool }
    let jobs = std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(16));
    let upn = up as usize;
    let check_one = move |owner: &crate::borrowck::body::CheckedBody, generic: bool| -> Outcome {
        let uu = unsafe { &*(upn as *const Unit) };
        let (prog, tcx) = (&uu.prog, &uu.tcx);
        let d = owner.owner;
        if std::env::var_os("MAKEPAD_RUN_CHECK_TRACE").is_some() { std::eprintln!("makepad check: body {}", prog.def_path(d)); }
        let mut out = Outcome { errors: Vec::new(), diags: Vec::new(), seed: None, retry: false };
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // the syntax-level lints do not wait for the types (rustc runs them before analysis)
            relax_types::lint_early::check_def(prog, tcx, d);
            let body = tcx.typeck(prog, d);
            // library stability (E0658) of what the body names
            let gates = relax_types::feature_gate::check_stability(prog, &body);
            let inner = || {
            if !body.errors.is_empty() {
                return (body.errors.to_vec(), Vec::new());
            }
            relax_types::lint::check_literals(prog, tcx, &body);
            relax_types::lint_pass::check_body(prog, tcx, &body);
            let unsafety = relax_types::unsafety::check_body(prog, tcx, &body);
            if !unsafety.is_empty() {
                return (unsafety, Vec::new());
            }
            // E0004 / E0005 and the pattern lints (rustc check_match: its errors stop the body
            // before borrowck); an internal failure of the analysis reports nothing
            let pats = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| relax_types::pattern_analysis::check_body(prog, tcx, &body)));
            match pats {
                Ok(e) if !e.is_empty() => return (e, Vec::new()),
                Ok(_) => {}
                Err(p) if p.is::<crate::preempt::NeedsLoad>() || crate::preempt::is_preempted(&*p) => std::panic::resume_unwind(p),
                Err(_) => {
                    if std::env::var_os("MAKEPAD_RUN_CHECK_TRACE").is_some() { std::eprintln!("makepad check: pattern analysis failed in {}", prog.def_path(d)); }
                }
            }
            if !crate::borrowck::enabled(options) {
                return (Vec::new(), Vec::new());
            }
            let v = crate::borrowck::check_body(&crate::tcx::EvalCx::new(prog, tcx, crate::tcx::TypingEnv::fully_monomorphized(prog), &crate::tcx::EvalState::new()), prog, tcx, &body);
            crate::borrowck::note_verdict(&v);
            if std::env::var_os("MAKEPAD_RUN_BORROWCK_UNCHECKED").is_some() {
                for x in &v.unchecked { eprintln!("makepad check: borrowck unchecked: {}", x); }
            }
            let args: Vec<_> = if owner.kind == crate::borrowck::body::BodyKind::Function && !generic { Vec::new() } else { body.args.to_vec() };
            let diags = crate::borrowck::lazy::report_checked(prog, 0, tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &args, v.errors, crate::borrowck::event::Origin::Check).iter().map(|e| e.error.to_diagnostic()).collect();
            (Vec::new(), diags)
            };
            let (mut errs, diags) = inner();
            errs.extend(gates);
            (errs, diags)
        }));
        match r {
            Ok((errs, diags)) => {
                if errs.is_empty() && owner.kind == crate::borrowck::body::BodyKind::Function && !generic {
                    out.seed = Some(d);
                }
                out.errors = errs;
                out.diags = diags;
            }
            // a module the check did not load (another crate's skipped file): checked again
            // once every file is loaded
            Err(p) if p.is::<crate::preempt::NeedsLoad>() => out.retry = true,
            // a constant it needed was asked of the compiler's owner thread while an earlier
            // request held it up (owner.rs): checked again
            Err(p) if crate::preempt::is_preempted(&*p) => out.retry = true,
            Err(_) => {
                let def = prog.def(d);
                let pos = prog.files[def.file as usize].ast.item(def.item).lo;
                out.errors.push(prog.record_error(def.file, pos, "typeck", format!("internal compiler error while checking {} -- makepad run bug, report it{}", prog.def_path(d), jit::take_ice_location())));
            }
        }
        // an error may be a name in a file the app check left unloaded: settled with all loaded
        if !out.errors.is_empty() { out.retry = true; }
        out
    };
    let t_par = t_check.elapsed();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut outcomes: Vec<(usize, Outcome)> = Vec::new();
    {
        let results = relax_session::sync::Mutex::new(Vec::with_capacity(fns.len()));
        let (fns, next, results, check_one) = (&fns, &next, &results, &check_one);
        std::thread::scope(|sc| {
            for w in 0..jobs {
                std::thread::Builder::new().name(format!("run-check-ty-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                    let mut local = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((owner, generic)) = fns.get(i) else { break };
                        local.push((i, check_one(owner, *generic)));
                        // return the allocator's free pages as the check goes (Stage held 4.6 GB)
                        if i % 20_000 == 19_999 { crate::memstat::trim(); }
                    }
                    results.lock().unwrap().extend(local);
                }).expect("check worker");
            }
        });
        outcomes.extend(results.lock().unwrap().drain(..));
    }
    if std::env::var_os("MAKEPAD_RUN_CHECK_TIMES").is_some() { std::eprintln!("makepad check: owners {:.2} s, anon consts {:.2} s, bodies {:.2} s ({} jobs)", t_owners.as_secs_f64(), (t_par - t_owners).as_secs_f64(), (t_check.elapsed() - t_par).as_secs_f64(), jobs); }
    outcomes.sort_by_key(|(i, _)| *i);
    // owners that failed or met an unloaded module: every skipped file loaded, checked again
    // (only when some other crate's file is still skipped: else the first answer stands)
    let skipped_left = unsafe { !(&*up).prog.skipped_files.is_empty() };
    if skipped_left && outcomes.iter().any(|(_, o)| o.retry) {
        unsafe { jit::load_all_skipped(up) };
        for (i, o) in outcomes.iter_mut() {
            if o.retry {
                let (owner, generic) = &fns[*i];
                *o = check_one(owner, *generic);
            }
        }
    }
    let mut seeds = Vec::new();
    for (_, o) in outcomes {
        errors.extend(o.errors);
        borrow_diagnostics.extend(o.diags);
        if let Some(d) = o.seed { seeds.push(d); }
    }
    let checked = seeds.len();
    relax_types::lint_dead::finish(&u.prog, &u.tcx, &own, root);
    relax_types::lint_early::finish(&u.prog, &own);
    errors.extend(static_self_access(up, &own));
    // the feature gates (E0658 and friends): items and signatures, then the loaded bodies
    for &k in crates {
        errors.extend(relax_types::feature_gate::check_crate(unsafe { &(*up).prog }, unsafe { &(*up).tcx }, k));
        errors.extend(relax_types::feature_gate::check_bodies(unsafe { &(*up).prog }, unsafe { &(*up).tcx }, k));
        errors.extend(relax_types::feature_gate::check_imports(unsafe { &(*up).prog }, k));
    }
    // rustc_hir_analysis check_crate: every static and every non-generic, non-associated const
    // item of the checked crates is evaluated, used or not; one that cannot be evaluated is an
    // E0080 (the unit notes it, reported below with the builds' other notes)
    eval_items(up, &own);
    // rustc_monomorphize: a program's `main` and what it reaches, instantiated -- the constants
    // generic code evaluates there (post-monomorphization E0080) -- in a check of a program
    if options.const_fuel && !compile && errors.is_empty() && borrow_diagnostics.is_empty() && unsafe { (&*up).errors.is_empty() } { mono_check(up, &own, root); }
    let t_front = t_check.elapsed();
    // the type/borrow check's freed memory goes back before the compile starts (Stage: 4.6 GB
    // of malloc free pages held after it)
    if compile { unsafe {
        let _job = jit::compile_job();
        let mut d = crate::owner::call(crate::owner::Class::App, || jit::settle_drop(up));
        d.release_rir();
    } crate::memstat::trim(); }
    if crate::memstat::enabled() {
        crate::memstat::print("check: type/borrow checked", &crate::memreport::unit_sizes(unsafe { &*up }));
    }
    for e in errors.iter_mut() {
        if is_internal(unsafe { &*up }, &own, e) && !e.contains(": internal:") && !e.starts_with("internal: ") {
            *e = label_internal(e);
        }
    }
    // every seed, and every instance, closure and glue slot the compiled code reaches (generic
    // callees, drop glue, vtable methods, std), compiled to machine code by the run's per-fn
    // compile until no new slots appear
    let failures = if compile { unsafe { compile_all(up, &seeds) } } else { Vec::new() };
    let n_failed = failures.len();
    // what the builds reported besides their own failures: constants that could not be
    // evaluated (CTFE panics), recorded instead of ending the check
    for e in unsafe { (&(*up).errors).iter().filter(|e| !e.starts_with("compile error in ")).cloned().collect::<Vec<_>>() } {
        errors.push(if is_internal(unsafe { &*up }, &own, &e) { label_internal(&e) } else { e });
    }
    for (id, e) in failures {
        let name = unsafe { (&(*up).fns)[id as usize].name.clone() };
        report(up, id, &name, &e, "compile", &own, &mut errors);
    }
    // MAKEPAD_RUN_BORROWCK_INSTANCES=1: every reached instance is borrow-checked too (the background
    // checker's view: concrete type arguments, all crates)
    if crate::borrowck::enabled(options) && options.borrowck_instances {
        for id in 0..unsafe { (&(*up).fns).len() } {
            let key = unsafe { (&(*up).fns)[id].key.clone() };
            let FnKey::Inst(d, args) = key else { continue };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
                let uu = &mut *up;
                let state = crate::tcx::EvalState::new();
                let cx = crate::tcx::EvalCx::new(&uu.prog, &uu.tcx, crate::tcx::TypingEnv::fully_monomorphized(&uu.prog), &state);
                let body = crate::typeck::instantiate_body(&cx, &uu.prog, &uu.tcx, d, &args);
                if !body.errors.is_empty() {
                    return;
                }
                let v = crate::borrowck::check_body(&crate::tcx::EvalCx::new(&uu.prog, &uu.tcx, crate::tcx::TypingEnv::fully_monomorphized(&uu.prog), &crate::tcx::EvalState::new()), &uu.prog, &uu.tcx, &body);
                crate::borrowck::note_verdict(&v);
                borrow_diagnostics.extend(crate::borrowck::lazy::report_checked(&uu.prog, 0, uu.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst), d, &body.args, v.errors, crate::borrowck::event::Origin::Check).iter().map(|e| e.error.to_diagnostic()));
            }));
            let _ = r;
        }
    }
    unsafe {
        // a delayed bug no error explains (rustc: an ICE at session end) is one internal failure
        // of the check, never the end of it
        // rustc DiagCtxt::flush_delayed: delayed bugs are ICEs only when no error was emitted
        let explained = !errors.is_empty() || !borrow_diagnostics.is_empty() || !(*up).errors.is_empty() || (*up).query_errors.has_errors();
        for bug in (*up).query_errors.take_delayed() {
            if !explained { errors.push(format!("internal: delayed bug at the end of the check: {}", bug)); }
        }
        if !errors.is_empty() { (*up).query_errors.note_reported_error(); }
        (*up).flush_query_errors(true);
        errors.extend_from_slice(&(*up).errors);
    }
    let t_compile = t_check.elapsed() - t_front;
    if crate::memstat::enabled() {
        crate::memstat::print("check", &crate::memreport::unit_sizes(unsafe { &*up }));
    }
    if compile {
        eprintln!("makepad check: {} crates checked, {} fns compiled from {} seeds, {} failed builds; type/borrow check {:.1} s, compile {:.1} s", crates.len(), unsafe { (&(*up).fns).len() }, seeds.len(), n_failed, t_front.as_secs_f64(), t_compile.as_secs_f64());
    } else {
        eprintln!("makepad check: {} crates type- and borrow-checked in {:.1} s", crates.len(), t_front.as_secs_f64());
    }
    relax_types::lint_levels::finish(&u.prog);
    let (lint_errors, mut lint_warnings) = relax_types::lint::take();
    errors.extend(lint_errors);
    lint_warnings.sort_by_cached_key(|e| error_position(e));
    lint_warnings.dedup();
    if emit && !crate::diag::enabled() {
        for w in &lint_warnings { std::eprintln!("{}", w); }
    }
    // rustc's order: by file, then position (a plain string sort put line 10 before line 3)
    errors.sort_by_cached_key(|e| error_position(e));
    errors.dedup();
    // a follow-on failure (a body or signature that could not be checked because of an error
    // already reported) is no error of its own: rustc reports only the cause
    if errors.iter().any(|e| !is_follow_on(e)) {
        errors.retain(|e| !is_follow_on(e));
    }
    for e in errors.iter().filter(|_| emit) {
        if crate::diag::enabled() {
            crate::diag::emit(&crate::diag::from_legacy(e, "typeck"));
        } else { println!("{}", e); }
    }
    crate::borrowck::finish_check(unsafe { &(*up).prog });
    let borrow_count = console.finish();
    eprintln!("makepad check: {} body owners of the checked crates, {} compiled in all, {} errors", n_own, unsafe { (*up).stats.compiled }, errors.len() + borrow_count);
    {
        let st = unsafe { &(*up).stats };
        eprintln!(
            "makepad check: cpu typeck {} ms, lower {} ms, opt {} ms, codegen {} ms; {} RIR insts, {} KB code",
            st.typeck_ns / 1_000_000,
            st.lower_ns / 1_000_000,
            st.opt_ns / 1_000_000,
            st.codegen_ns / 1_000_000,
            st.rir_insts,
            st.code_bytes / 1024
        );
        eprintln!("makepad check: constant initializers: {} evaluated to data, {} compiled and run; evaluation cpu {:.1} ms", st.ctfe_data, st.ctfe_code, st.ctfe_ns as f64 / 1e6);
    }
    unsafe { crate::jit::backend_bench(up) };
    if std::env::var("MAKEPAD_RUN_PASS_TIMES").is_ok() || std::env::var("MAKEPAD_RUN_OPT_HIST").is_ok() {
        let mut passes = String::new();
        for i in 0..relax_backend::stats::PASS_NAMES.len() {
            passes.push_str(&format!("{} {:.0}  ", relax_backend::stats::PASS_NAMES[i], relax_backend::stats::PASS_NS[i].load(std::sync::atomic::Ordering::Relaxed) as f64 / 1e6));
        }
        eprintln!("makepad check: passes (cpu ms): {}", passes);
        eprintln!("makepad check: opt ms by size (<16,<64,<256,<1k,<4k,more) {:?} fns {:?}", relax_codegen::opt::HIST[..6].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000).collect::<Vec<_>>(), relax_codegen::opt::HIST[6..].iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed)).collect::<Vec<_>>());
    }
    let _ = checked;
    crate::relaxed::finish_check();
    let mut diagnostics: Vec<_> = errors.iter().map(|e| crate::diag::from_legacy(e, "typeck")).collect();
    diagnostics.extend(borrow_diagnostics);
    (i32::from(!errors.is_empty() || borrow_count != 0), diagnostics)
}

/// Compiles every seed fn, then every slot the compiled code creates (callees, closures, glue,
/// vtable methods), each through the run's own per-fn compile (`jit::compile_slot`: front end
/// on the owner thread, optimisation and code generation off it), on MAKEPAD_RUN_JOBS threads until
/// no slot is left. Returns every failed build: (slot, error).
unsafe fn compile_all(up: *mut Unit, seeds: &[crate::program::DefId]) -> Vec<(u32, String)> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    *jit::CHECK_FAILURES.lock().unwrap_or_else(|p| p.into_inner()) = Some(Vec::new());
    // as a run: the front end on the owner thread, optimisation and code generation off it
    // (and the run's speculative callee builds)
    jit::enable_pipelining(&(*up).prog.session.clone().options);
    // (slots made before the seeds -- the root's main, constants' readers -- are compiled too)
    crate::owner::call(crate::owner::Class::App, || {
        for &d in seeds {
            (&mut *up).fn_id(FnKey::Inst(d, Vec::new()));
        }
    });
    // every in-flight build holds its front end's and back end's state: at most 8 by default
    let jobs = std::env::var("MAKEPAD_RUN_JOBS").ok().and_then(|v| v.parse::<usize>().ok()).filter(|n| *n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get()).min(8));
    // the run's reclamation (jit::settle_drop: RIR, checked bodies, templates of compiled
    // slots) once the process grew SETTLE_GROWTH since the last one (looked at every 2,000
    // compiled slots), so a whole-program check keeps the working set of a run, not every body
    // it ever built (Stage went past 18 GB without it). Dropped RIR is rebuilt for inlining:
    // settling every 20,000 slots made Scope's compile 4x slower.
    const SETTLE_GROWTH: u64 = 3 << 30;
    let compiled = AtomicUsize::new(0);
    let settled_at = AtomicUsize::new(0);
    let settled_rss = std::sync::atomic::AtomicU64::new(crate::memstat::resident() as u64);
    let next = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let panics: relax_session::sync::Mutex<Vec<(u32, String)>> = relax_session::sync::Mutex::new(Vec::new());
    let upn = up as usize;
    std::thread::scope(|sc| {
        for w in 0..jobs {
            let (next, active, panics, compiled, settled_at, settled_rss) = (&next, &active, &panics, &compiled, &settled_at, &settled_rss);
            std::thread::Builder::new().name(format!("run-check-{}", w)).stack_size(256 << 20).spawn_scoped(sc, move || { let _z = crate::zone_alloc::compiler_thread();
                let up = upn as *mut Unit;
                loop {
                    // the next slot not compiled yet; done when none is left and no worker is
                    // compiling (a compile in flight may still make new slots)
                    active.fetch_add(1, Ordering::SeqCst);
                    let pick = {
                        let _job = jit::compile_job();
                        crate::owner::call(crate::owner::Class::App, || {
                            let n = (&(*up).fns).len();
                            let mut pick = None;
                            loop {
                                let i = next.load(Ordering::SeqCst);
                                if i >= n { break; }
                                if next.compare_exchange(i, i + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                                    let f = &(&(*up).fns)[i];
                                    if !f.compiled {
                                        pick = Some(i as u32);
                                        break;
                                    }
                                }
                            }
                            pick
                        })
                    };
                    let Some(id) = pick else {
                        active.fetch_sub(1, Ordering::SeqCst);
                        if active.load(Ordering::SeqCst) == 0 && next.load(Ordering::SeqCst) >= crate::owner::call(crate::owner::Class::App, || (&(*up).fns).len()) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                        continue;
                    };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::compile_slot(up, id)));
                    if let Err(p) = r {
                        let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
                        panics.lock().unwrap().push((id, format!("internal compiler error: {}{}", msg, jit::take_ice_location())));
                    }
                    active.fetch_sub(1, Ordering::SeqCst);
                    let n = compiled.fetch_add(1, Ordering::Relaxed) + 1;
                    let at = settled_at.load(Ordering::Relaxed);
                    if n >= at + 2_000 && settled_at.compare_exchange(at, n, Ordering::AcqRel, Ordering::Relaxed).is_ok()
                        && crate::memstat::resident() as u64 > settled_rss.load(Ordering::Relaxed) + SETTLE_GROWTH {
                        let _job = jit::compile_job();
                        let mut d = crate::owner::call(crate::owner::Class::App, || jit::settle_drop(up));
                        d.release_rir();
                        crate::memstat::trim();
                        settled_rss.store(crate::memstat::resident() as u64, Ordering::Relaxed);
                    }
                }
            }).expect("check worker");
        }
    });
    let mut out = jit::CHECK_FAILURES.lock().unwrap_or_else(|p| p.into_inner()).take().unwrap_or_default();
    out.extend(panics.into_inner().unwrap());
    out
}

/// Is a failure a makepad run bug rather than an error rustc would report? A compiler panic, an
/// internal error, an error without an error code, or one located outside the checked crates
/// (std or a dependency: rustc builds those).
fn is_internal(u: &Unit, own: &[bool], e: &str) -> bool {
    let first = e.lines().next().unwrap_or("");
    if first.contains("internal compiler error") || first.contains("internal:") || first.contains("makepad run bug") {
        return true;
    }
    // `path:line:col: E0123: ..`
    let mut it = first.splitn(4, ':');
    let (Some(path), Some(l), Some(c), Some(rest)) = (it.next(), it.next(), it.next(), it.next()) else { return true };
    if l.trim().parse::<u32>().is_err() || c.trim().parse::<u32>().is_err() {
        return true;
    }
    let rest = rest.trim_start();
    // (an error rustc words without a code: Program::record_uncoded_error)
    let coded = rest.starts_with(relax_front::diagnostics::UNCODED_PREFIX) || (rest.len() > 5 && rest.starts_with('E') && rest[1..5].bytes().all(|b| b.is_ascii_digit()));
    if !coded {
        return true;
    }
    match u.prog.file_paths.iter().position(|p| p == path) {
        Some(i) => !u.prog.file_crate.get(i).is_some_and(|k| own.get(*k as usize).copied().unwrap_or(false)),
        None => true,
    }
}

/// `path:line:col: internal: msg` for a makepad run bug (the error keeps its own text otherwise).
fn label_internal(e: &str) -> String {
    let mut it = e.splitn(4, ':');
    if let (Some(p), Some(l), Some(c), Some(rest)) = (it.next(), it.next(), it.next(), it.next()) {
        if l.trim().parse::<u32>().is_ok() && c.trim().parse::<u32>().is_ok() {
            return format!("{}:{}:{}: internal:{}", p, l, c, rest);
        }
    }
    format!("internal: {}", e)
}

/// One failed build as an error line: its location (the error's own, else the fn's), the
/// message's first line, the instance it failed for, and `internal` for a makepad run bug.
fn report(up: *mut Unit, id: u32, name: &str, e: &str, phase: &'static str, own: &[bool], errors: &mut Vec<String>) {
    let u = unsafe { &*up };
    let mut first = e.lines().next().unwrap_or("").to_string();
    // an error without a location takes its fn's
    let located = {
        let mut it = first.splitn(4, ':');
        matches!((it.next(), it.next(), it.next()), (Some(_), Some(l), Some(c)) if l.trim().parse::<u32>().is_ok() && c.trim().parse::<u32>().is_ok())
    };
    if !located {
        if let Some((file, pos)) = function_origin(u, id) {
            let path = &u.prog.file_paths[file as usize];
            let (l, c) = crate::lexer::line_col(&u.prog.files[file as usize].src, pos);
            first = format!("{}:{}:{}: {}", path, l, c, first);
        }
    }
    let internal = is_internal(u, own, &first);
    // the type check of the fn reported it already (a non-generic body checked before compiling)
    let shown = if internal { label_internal(&first) } else { first.clone() };
    if errors.iter().any(|x| *x == shown) {
        return;
    }
    let mut n = name.to_string();
    if let crate::jit::FnKey::Inst(_, args) = &u.fns[id as usize].key {
        if !args.is_empty() && !n.contains('<') {
            let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect();
            n = format!("{}<{}>", n, a.join(", "));
        }
    }
    let line = format!("{} [in {}]", shown, n);
    if crate::diag::enabled() {
        let mut d = crate::diag::recorded(e).or_else(|| crate::diag::recorded(&first)).unwrap_or_else(|| crate::diag::from_legacy(&first, phase));
        d.notes.push(format!("in {}", n));
        if internal { d.notes.push("internal: a makepad run bug, not an error in the program".to_string()); }
        crate::diag::record(line.clone(), d);
    }
    errors.push(line);
}

fn function_origin(u: &Unit, mut id: u32) -> Option<(u32,u32)> {
    for _ in 0..64 {
        match &u.fns[id as usize].key {
            FnKey::Inst(d,_) => { let d = u.prog.def(*d); return Some((d.file,u.prog.files[d.file as usize].ast.item(d.item).lo)); }
            FnKey::Closure(f,e,_) => return Some((*f,u.prog.files[*f as usize].ast.expr(crate::ast::ExprId(*e)).lo)),
            FnKey::Sandbox(parent) | FnKey::Plain(parent) => id = *parent,
            FnKey::Shim(shim) => { let d = u.prog.def(shim.kind.def_id()); return Some((d.file, u.prog.files[d.file as usize].ast.item(d.item).lo)); }
            FnKey::Glue(_,t) => {
                if let crate::types::TyKind::Adt(d,_, _) = u.tcx.tys.kind(*t) {
                    let d = u.prog.def(*d); return Some((d.file,u.prog.files[d.file as usize].ast.item(d.item).lo));
                }
                return None;
            }
        }
    }
    None
}

/// (path, line, col, text) of an error line `path:line:col: ...` (no location: sorts first).
fn error_position(e: &str) -> (String, u32, u32, String) {
    let first = e.lines().next().unwrap_or("");
    for (i, _) in first.match_indices(':') {
        let mut it = first[i + 1..].splitn(3, ':');
        if let (Some(l), Some(c)) = (it.next(), it.next()) {
            if let (Ok(l), Ok(c)) = (l.trim().parse::<u32>(), c.trim().parse::<u32>()) {
                return (first[..i].to_string(), l, c, e.to_string());
            }
        }
    }
    (String::new(), 0, 0, e.to_string())
}

/// A check that could not run because of an error reported elsewhere (typeck's cascade notes).
fn is_follow_on(e: &str) -> bool {
    let first = e.lines().next().unwrap_or("");
    first.contains("type checking failed due to previous errors")
        || first.contains("signature has unresolved types -- fix the signature errors first")
        || first.contains("has unresolved types in its signature -- fix its signature errors first")
}

/// Evaluates the checked crates' statics and non-generic const items (rustc evaluates them in
/// `cargo check`, used or not). A body with type errors is skipped: those errors are reported.
fn eval_items(up: *mut Unit, own: &[bool]) {
    let u = unsafe { &mut *up };
    let items: Vec<(crate::program::DefId, bool)> = (0..u.prog.defs.len())
        .filter(|&i| own[u.prog.defs[i].krate as usize])
        .map(|i| crate::program::DefId(i as u32))
        .filter_map(|d| match u.prog.def(d).kind {
            DefKind::Static => Some((d, true)),
            DefKind::Const if u.tcx.generic_indices_of(&u.prog, d).types_consts.is_empty() => Some((d, false)),
            _ => None,
        })
        .filter(|(d, _)| relax_types::const_eval::const_body(&u.prog, *d).is_some())
        .collect();
    let _job = jit::compile_job();
    for (d, is_static) in items {
        if !u.tcx.typeck(&u.prog, d).errors.is_empty() { continue; }
        let before = unsafe { (&*up).errors.len() };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::owner::call(crate::owner::Class::App, || unsafe {
            let u = &mut *up;
            if is_static { u.static_addr(d); } else { u.const_addr(d, Vec::new()); }
        })));
        // a foreign static the check does not link is not the program's error
        let u = unsafe { &mut *up };
        let mut i = before;
        while i < u.errors.len() {
            if u.errors[i].starts_with("compile error in ") && u.errors[i].contains("not found in the loaded libraries") { u.errors.remove(i); } else { i += 1; }
        }
    }
}

/// rustc's collect_and_partition_mono_items for a binary, reduced to its constants and its recursion
/// limit: every instance of the checked crates' fns reachable from `main` is lowered (not compiled),
/// which evaluates the constants it names with its type arguments; a failing one is reported
/// (E0080) as rustc's codegen reports it. An instance more than `recursion_limit` instances of the same
/// definition deep (polymorphic recursion that grows its types) is "reached the recursion limit while
/// instantiating" (rustc_monomorphize collector `check_recursion_limit`). Library code is not entered:
/// its constants are not the program's diagnostics.
fn mono_check(up: *mut Unit, own: &[bool], root: u32) {
    let u = unsafe { &mut *up };
    let main = (0..u.prog.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|&d| {
        let def = u.prog.def(d);
        def.kind == DefKind::Fn && def.krate == root && u.prog.name(d) == "main"
            && def.parent == u.prog.mods[u.prog.crates[root as usize].root_mod as usize].def
    });
    let Some(main) = main else { return };
    let limit = u.prog.crates[root as usize].recursion_limit as usize;
    let _job = jit::compile_job();
    // (instance, the instance it was reached from): the chain of a reached instance is its DFS stack
    let mut work: Vec<(u32, Option<u32>)> = vec![(crate::owner::call(crate::owner::Class::App, || unsafe { (&mut *up).fn_id(FnKey::Inst(main, Vec::new())) }), None)];
    let mut parents: std::collections::HashMap<u32, Option<u32>> = std::collections::HashMap::new();
    while let Some((id, parent)) = work.pop() {
        if parents.contains_key(&id) { continue; }
        parents.insert(id, parent);
        let (def_of, mine) = {
            let u = unsafe { &*up };
            let def_of = |i: u32| match &u.fns[i as usize].key { FnKey::Inst(d, _) => Some(*d), _ => None };
            (def_of(id), match &u.fns[id as usize].key { FnKey::Inst(d, _) => own[u.prog.def(*d).krate as usize], _ => false })
        };
        // collector.rs check_recursion_limit: the active instances of this definition on the stack
        let mut same_def = 0usize;
        if let Some(d) = def_of {
            let u = unsafe { &*up };
            let mut at = parent;
            while let Some(a) = at {
                if matches!(&u.fns[a as usize].key, FnKey::Inst(x, _) if *x == d) { same_def += 1; }
                at = parents.get(&a).copied().flatten();
            }
            if same_def > limit {
                let def = u.prog.def(d);
                let pos = u.prog.files[def.file as usize].ast.item(def.item).lo;
                let name = u.fns[id as usize].name.clone();
                let msg = u.prog.record_uncoded_error(def.file, pos, "mono", format!("reached the recursion limit while instantiating `{}`", name));
                unsafe { (&mut *up).errors.push(msg); }
                continue;
            }
        }
        let callees = crate::owner::call(crate::owner::Class::App, || unsafe {
            if !mine { return Vec::new(); }
            // (a fn taking or returning a value of hundreds of MB is not lowered: its frame would
            // be allocated for real)
            // (instances over deeply nested types -- closures wrapped in closures -- cost more to
            // lower than a constants check is worth; one that recurses into itself is followed to the limit)
            if let FnKey::Inst(_, args) = &(&*up).fns[id as usize].key {
                if same_def < 2 && args.iter().any(|t| (&*up).tcx.tys.depth(*t) > 6) { return Vec::new(); }
            }
            if let FnKey::Inst(d, _) = &(&*up).fns[id as usize].key {
                let uu = &*up;
                if uu.tcx.typeck(&uu.prog, *d).expr_ty.iter().any(|t| uu.tcx.tys.depth(*t) > 8) { return Vec::new(); }
            }
            let huge = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if let FnKey::Inst(d, args) = &(&*up).fns[id as usize].key {
                    let (d, args) = (*d, args.clone());
                    let uu = &mut *up;
                    if let Some(sig) = uu.tcx.fn_sig(&uu.prog, d).cloned() {
                        let env = crate::tcx::TypingEnv::fully_monomorphized(&uu.prog);
                        for t in sig.params.iter().chain(std::iter::once(&sig.ret)) {
                            let t = uu.tcx.tys.subst_erasing_regions(*t, &args);
                            let Ok(t) = uu.tcx.try_normalize_erasing_regions(&uu.prog, env, t) else { return true };
                            if uu.tcx.is_coroutine(t) { continue; }
                            if uu.layout_of(t).size > (1 << 28) { return true; }
                        }
                    }
                }
                false
            })).unwrap_or(true);
            if huge { return Vec::new(); }
            let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| jit::build_rir(up, id)));
            // (the lowering's own limit on how deep the type arguments of an instance may nest is the same one)
            if let Ok(Err(msg)) = &built {
                if msg.starts_with("reached the recursion limit while instantiating") {
                    let u = &mut *up;
                    if let FnKey::Inst(d, _) = &u.fns[id as usize].key {
                        let def = u.prog.def(*d);
                        let pos = u.prog.files[def.file as usize].ast.item(def.item).lo;
                        let msg = u.prog.record_uncoded_error(def.file, pos, "mono", msg.clone());
                        u.errors.push(msg);
                    }
                }
            }
            if !matches!(built, Ok(Ok(()))) { return Vec::new(); }
            let mut out = Vec::new();
            if let Some(f) = &(&*up).fns[id as usize].rir {
                for b in &f.blocks {
                    for i in &b.insts {
                        match i {
                            relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(c) | relax_backend::rir::Callee::CFn(c, _), _, _) => out.push(*c),
                            _ => {}
                        }
                    }
                }
            }
            out
        });
        work.extend(callees.into_iter().map(|c| (c, Some(id))));
    }
}

/// rustc_const_eval interpret/memory.rs: "encountered static that tried to access itself during
/// initialization" -- the initializer of a static reads the static's own value (`static A: u32 = A`).
/// Taking its address (`&A`, `&A.0`, `addr_of!`) is fine. Initializers holding closures are left
/// alone (a closure body runs later).
fn static_self_access(up: *mut Unit, own: &[bool]) -> Vec<String> {
    use crate::ast::ExprKind;
    use crate::typeck::Res;
    let u = unsafe { &*up };
    let mut out = Vec::new();
    for i in 0..u.prog.defs.len() {
        let d = crate::program::DefId(i as u32);
        let def = u.prog.def(d);
        if def.kind != DefKind::Static || !own[def.krate as usize] { continue; }
        if relax_types::const_eval::const_body(&u.prog, d).is_none() { continue; }
        let Ok(body) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.tcx.typeck(&u.prog, d))) else { continue };
        if !body.errors.is_empty() { continue; }
        let ast = &u.prog.files[body.file as usize].ast;
        let ids: Vec<crate::ast::ExprId> = (0..body.expr_ty.len()).map(|k| crate::ast::ExprId(if k < body.expr_n1 as usize { body.expr_lo + k as u32 } else { body.expr_lo2 + k as u32 - body.expr_n1 })).collect();
        if ids.iter().any(|e| matches!(ast.expr(*e).kind, ExprKind::Closure(..) | ExprKind::Async(..))) { continue; }
        let mut addr_only = std::collections::HashSet::new();
        for e in &ids {
            if let ExprKind::AddrOf(_, _, x) = ast.expr(*e).kind {
                let mut x = x;
                loop {
                    addr_only.insert(x.0);
                    match ast.expr(x).kind {
                        ExprKind::Paren(y) | ExprKind::Field(y, _) | ExprKind::TupleField(y, _) | ExprKind::Index(y, _) => x = y,
                        _ => break,
                    }
                }
            }
        }
        // a write through a place rooted in another static
        for e in &ids {
            let (ExprKind::Assign(lhs, _) | ExprKind::AssignOp(_, lhs, _)) = ast.expr(*e).kind else { continue };
            let mut x = lhs;
            loop {
                match ast.expr(x).kind {
                    ExprKind::Paren(y) | ExprKind::Field(y, _) | ExprKind::TupleField(y, _) | ExprKind::Index(y, _) | ExprKind::AddrOf(_, _, y) | ExprKind::Unary(crate::ast::UnOp::Deref, y) => x = y,
                    _ => break,
                }
            }
            if matches!(ast.expr(x).kind, ExprKind::Path(_)) {
                if let Some(Res::Def(t)) = body.res.get(&x.0) {
                    if *t != d && u.prog.def(*t).kind == DefKind::Static {
                        out.push(u.prog.record_error(body.file, ast.expr(*e).lo, "typeck", "E0080: modifying a static's initial value from another static's initializer".to_string()));
                    }
                }
            }
        }
        for e in &ids {
            if matches!(ast.expr(*e).kind, ExprKind::Path(_)) && !addr_only.contains(&e.0) && matches!(body.res.get(&e.0), Some(Res::Def(t)) if *t == d) {
                out.push(u.prog.record_error(body.file, ast.expr(*e).lo, "typeck", "E0080: encountered static that tried to access itself during initialization".to_string()));
                break;
            }
        }
    }
    out
}
