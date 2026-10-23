//! Settling must be invisible to generated O1 code and to later closure rebuilds.
use super::*;

#[test]
fn fn_entry_size() {
    isolated_test!(fn_entry_size);
    // The original layout measures the inline cost on the same target as FnEntry.
    #[allow(dead_code)]
    struct InlineFnEntry {
        key: FnKey,
        name: String,
        code: u64,
        code_len: u32,
        pc_map: PcMap,
        file: u32,
        prev: Vec<(u64, u32, PcMap, u32)>,
        compiled: bool,
        patch_gen: u32,
        rir: Option<std::sync::Arc<relax_backend::rir::Func>>,
        refs: u32,
        rir_refs: Box<[u32]>,
        entry_ref: AtomicBool,
        entry_ref_counted: bool,
        building: bool,
        build_err: Option<String>,
        inlined_into: Vec<u32>,
        build_ns: u64,
        cg_ns: u64,
        version: u32,
        retired: Vec<(u64, u32, PcMap)>,
        inflight: bool,
        spec: Option<(Vec<u8>, Vec<(u32, u64)>, Vec<relax_backend::rir::RelocAt>, bool)>,
        spec_state: u8,
        spec_depth: u8,
        baseline_hint: AtomicBool,
        hot_hint: AtomicBool,
        o3: AtomicBool,
        base_code: AtomicBool,
        unverified: AtomicBool,
        d3_lowered: AtomicBool,
        calls: Box<[u32]>,
    }
    let before = std::mem::size_of::<InlineFnEntry>();
    let after = std::mem::size_of::<FnEntry>();
    eprintln!("size_of::<FnEntry>(): {before} -> {after} bytes");
    assert!(after < before);
    #[cfg(target_pointer_width = "64")]
    assert_eq!(before - after, 152);
}

// Rounds 49-50: restarting leaves the running solver answers and native code intact.
#[test]
fn round47_const_fn_body_invalidates_solver_answer() {
    isolated_test!(round47_const_fn_body_invalidates_solver_answer);
    const CHILD: &str = "MAKEPAD_RUN_R47_CONST_FN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round47_const_fn_body_invalidates_solver_answer", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0")
            .output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    // rustc_const_eval/src/interpret/call.rs:805-823 loads the callee MIR:
    // CTFE and solver answers depend on const-fn bodies, not just const owners.
    crate::borrowck::lazy::disable_gandalf();
    let source = "#![no_std]\nconst fn width() -> u32 { 3 } const VALUE: u32 = width();";
    let path = std::env::temp_dir().join(format!("relax-r47-const-fn-{}.rs", std::process::id()));
    std::fs::write(&path, &source).unwrap();
    let mut session = crate::Session::default();
    session.options.ctfe = crate::options::Ctfe::Off;
    let session = std::sync::Arc::new(session);
    let mut prog = Program::new(&session);
    prog.load_crate("const_fn_edit", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let value = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "VALUE").then_some(DefId(i as u32))).unwrap();
    let file = prog.def(value).file;
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let accepts = |u: &Unit, bits| {
        use relax_types::next_solver::*;
        use crate::consts::{AliasConst, AliasConstKind, Const, ScalarInt, ValTree};
        let infcx = crate::infer::InferCtxt::new(&u.tcx, crate::tcx::TypingMode::PostAnalysis, Default::default());
        let expected = Const::new_value(&u.tcx.tys, ValTree::from_scalar_int(&u.tcx.tys, ScalarInt::try_from_uint(bits, 4).unwrap()), u.tcx.tys.u32_);
        let goal = Goal { param_env: ParamEnvId::EMPTY, predicate: Predicate::dummy(PredicateKind::Clause(ClauseKind::Projection(ProjectionClause {
            projection_term: AliasTerm::Const(AliasConst { kind: AliasConstKind::Free { def_id: value }, args: vec![].into() }), term: Term::Const(expected),
        }))) };
        infcx.evaluate_root_goal(&u.prog, goal, None).is_ok_and(|r| r.certainty.is_yes())
    };
    assert!(accepts(&u, 3));
    assert!(!accepts(&u, 4));
    assert!(u.stats.ctfe_code > 0, "must execute the real const function");
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file, path: path.to_string_lossy().into_owned(), src: source.replace("{ 3 }", "{ 4 }").into_bytes(),
    }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Restart, "{:?}", outcome.lines);
    assert!(accepts(&u, 3), "restart changed the running solver answer");
    assert!(!accepts(&u, 4), "restart published the candidate solver answer");
    std::fs::remove_file(path).unwrap();
}

// Round 50: both candidates restart without executing pending code. The invalid
// candidate is diagnosed separately by a cold check.
#[test]
fn round48_const_fn_tentative_check_uses_new_body() {
    isolated_test!(round48_const_fn_tentative_check_uses_new_body);
    const CHILD: &str = "MAKEPAD_RUN_R48_CONST_FN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round48_const_fn_tentative_check_uses_new_body", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0")
            .output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        let cold = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round48_const_fn_tentative_check_uses_new_body", "--nocapture", "--test-threads=1"])
            .env(CHILD, "cold").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0")
            .output().unwrap();
        assert!(cold.status.success(), "{}\n{}", String::from_utf8_lossy(&cold.stdout), String::from_utf8_lossy(&cold.stderr));
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    let source = "#![no_std]\nconst fn width() -> usize { 3 }\nfn probe() { let _: [u8; width()] = [0; 3]; }\nfn read(out: *mut usize) { unsafe { *out = width(); } }";
    let cold = std::env::var(CHILD).unwrap() == "cold";
    let source = if cold { source.replace("{ 3 }", "{ 5 }").replace("[0; 3]", "[0; 4]") } else { source.into() };
    let path = std::env::temp_dir().join(format!("relax-r48-const-fn-{}.rs", std::process::id()));
    std::fs::write(&path, &source).unwrap();
    let mut session = crate::Session::default();
    session.options.ctfe = crate::options::Ctfe::Off;
    session.options.opt = vec!["-inline".into()];
    let session = std::sync::Arc::new(session);
    let mut prog = Program::new(&session);
    prog.load_crate("tentative_const_fn", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let probe = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "probe").then_some(DefId(i as u32))).unwrap();
    let file = prog.def(probe).file;
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let before = u.tcx.typeck(&u.prog, probe);
    if cold {
        assert!(before.errors.iter().any(|e| e.contains("mismatched types")), "{:?}", before.errors);
        std::fs::remove_file(path).unwrap();
        return;
    }
    assert!(before.errors.is_empty(), "{:?}", before.errors);
    assert!(u.stats.ctfe_code > 0, "must execute the real const function before reload");
    let read = u.prog.defs.iter().enumerate().find_map(|(i, _)| (u.prog.name(DefId(i as u32)) == "read").then_some(DefId(i as u32))).unwrap();
    let slot = u.fn_id(FnKey::Inst(read, vec![]));
    let entry = u.entry(slot);
    let mut running = 0usize;
    unsafe { call_jit(up, entry, &mut running as *mut usize as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(running, 3);
    let executions = u.stats.ctfe_code;
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file, path: path.to_string_lossy().into_owned(), src: source.replace("{ 3 }", "{ 4 }").replace("[0; 3]", "[0; 4]").into_bytes(),
    }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Restart, "valid edit checked with stale const-fn code: {:?}", outcome.lines);
    assert_eq!(u.stats.ctfe_code, executions, "restart executed candidate CTFE");
    let after = u.tcx.typeck(&u.prog, probe);
    assert!(after.errors.is_empty(), "{:?}", after.errors);
    unsafe { call_jit(up, entry, &mut running as *mut usize as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(running, 3, "restart published the candidate callable");
    eprintln!("round50 valid: {:?}, running={running}, CTFE executions unchanged", outcome.rung);
    let candidate = source.replace("{ 3 }", "{ 5 }").replace("[0; 3]", "[0; 4]")
        .replace("fn probe() {", "fn probe() { let _retry = 0;");
    let refused = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file: u.prog.def(probe).file, path: path.to_string_lossy().into_owned(), src: candidate.into_bytes(),
    }]) };
    unsafe { call_jit(up, entry, &mut running as *mut usize as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(running, 3,
        "invalid edit published candidate const-fn code to the running program");
    assert_eq!(u.stats.ctfe_code, executions, "invalid edit executed candidate CTFE");
    eprintln!("round50 invalid: {:?}, running={running}, CTFE executions unchanged", refused.rung);
    std::fs::remove_file(path).unwrap();
    assert_eq!(refused.rung, crate::reload::Rung::Restart, "round50 requires restart of the CTFE-dependent invalid edit: {:?}", refused.lines);
}

#[test]
fn owner_front_write_serves_waiting_readers() {
    isolated_test!(owner_front_write_serves_waiting_readers);
    // A front reader waiting for the owner thread (a constant for its type check) holds its read
    // guard; the owner taking the front's write lock serves it meanwhile (front_write pumps), so
    // neither waits for the other.
    let session = std::sync::Arc::new(crate::Session::default());
    let prog = Program::new(&session);
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    let _u = Unit::new(prog, tcx, 0);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let _reader = front_read("owner-test");
        ready_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        crate::owner::call(crate::owner::Class::Job, || {});
    });
    ready_rx.recv().unwrap();
    // without the pump the two wait for each other forever (the test hangs, which the harness
    // reports); how long the serving takes is the machine's (a JIT-run test compiles on first use)
    crate::owner::call(crate::owner::Class::Program, || {
        let _writer = front_write();
    });
    reader.join().unwrap();
}

#[test]
fn allocator_swap_serves_background_finish() {
    isolated_test!(allocator_swap_serves_background_finish);
    // A background build's job (O2, speculation, O3) that reaches its finish while an allocator
    // swap pauses the jobs: the finish keeps the job running (Class::keeps_job), so the owner
    // serves it for the job's thread and the swap's wait ends. Held back, the swap waited forever.
    crate::owner::call(crate::owner::Class::Program, || {});
    let (began_tx, began_rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let job = std::thread::spawn(move || {
        let _job = compile_job();
        began_tx.send(()).unwrap();
        go_rx.recv().unwrap();
        crate::owner::publish(|| {});
        crate::owner::call(crate::owner::Class::Job, || {});
    });
    began_rx.recv().unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let pauser = std::thread::spawn(move || {
        pause_compile_jobs(false);
        done_tx.send(()).unwrap();
        resume_compile_jobs(false);
    });
    while !BG_PAUSE.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    go_tx.send(()).unwrap();
    assert!(done_rx.recv_timeout(std::time::Duration::from_secs(20)).is_ok(),
        "the allocator swap waits for a job whose finish the pause holds back");
    pauser.join().unwrap();
    job.join().unwrap();
}

#[test]
fn hot_o3_cache_roundtrip() {
    isolated_test!(hot_o3_cache_roundtrip);
    // Tiering is process-global: the second unit is a fresh launch, sharing only disk cache.
    const CHILD: &str = "MAKEPAD_RUN_HOT_O3_CACHE_TEST";
    const DIR: &str = "MAKEPAD_RUN_HOT_O3_CACHE_TEST_DIR";
    let Ok(phase) = std::env::var(CHILD) else {
        let dir = std::env::temp_dir().join(format!("relax-hot-o3-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("input.rs"), r#"
pub fn hot(out: &mut u32) {
    let mut i = 0u32;
    let mut sum = 0u32;
    while i < 200 { sum = sum.wrapping_add(i); i += 1; }
    *out = sum;
}
fn main() {}
"#).unwrap();
        for phase in ["cold", "warm"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::settle_tests::hot_o3_cache_roundtrip", "--nocapture", "--test-threads=1"])
                .env(CHILD, phase).env(DIR, &dir).env("MAKEPAD_RUN_SPEC", "0")
                .output().unwrap();
            assert!(output.status.success(), "{phase}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    crate::borrowck::lazy::disable_gandalf();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::path::PathBuf::from(std::env::var_os(DIR).unwrap());
    std::env::set_var("MAKEPAD_RUN_CACHE_DIR", dir.join("cache"));
    let session = std::sync::Arc::new(crate::Session::default());
    let (prog, core, krate) = crate::load_with_std(&session, dir.join("input.rs").to_str().unwrap(), "hot_o3_cache", false).unwrap();
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == "hot").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let mut entry = None;
    let mut first_slot = None;
    let mut initial_slot = None;
    let id = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(FnKey::Inst(def, Vec::new()));
        assert!(relax_level(&u, id) < 3, "the fixture must be unmarked");
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        if phase == "cold" {
            let first = unsafe { compile_fn(up, id) }.unwrap();
            let initial = u.install(id, &first.0, &first.2, first.1.clone(), 0).unwrap();
            let mut cache = u.cache.take().unwrap();
            cache.record_o2(&u, id, &first.0, &first.2, &first.1);
            assert!(cache.pending_bytes() > 0);
            u.cache = Some(cache);
            first_slot = Some(first);
            initial_slot = Some(initial);
        } else {
            entry = Some(unsafe { ready_code_locked(up, id) }.expect("hot O3 must link on the second launch"));
            assert!(u.fns[id as usize].o3.load(Ordering::Relaxed));
            assert_eq!(u.stats.tier1_cached, 1);
            assert_eq!(u.stats.tier1_installed, 0, "warm linking must not rebuild O3");
            assert!(u.fns[id as usize].rir.is_none());
            assert_eq!(unsafe { (*tier_rec(&u.slots, id).add(3)).load(Ordering::Acquire) }, T_QUEUED);
            assert!(TIER_QUEUE.lock().unwrap().is_empty());
            assert!(O2_QUEUE.lock().unwrap().is_empty());
        }
        id
    });
    if phase == "cold" {
        let (first, initial) = (first_slot.unwrap(), initial_slot.unwrap());
        let mut result = 0u32;
        unsafe { call_jit(up, initial, &mut result as *mut u32 as u64) }
            .unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, 19900);
        assert!(unsafe { compile_tier1(up, id) });
        let recorded = crate::owner::call(crate::owner::Class::Program, || {
            // (an O3 install's record is encoded in the record batches)
            u.flush_cache_records();
            let recorded = u.cache.as_ref().unwrap().stats.recorded;
            // A retier replaces the first O3 entry too.
            recorded
        });
        assert!(unsafe { compile_tier1(up, id) });
        let spill = crate::owner::call(crate::owner::Class::Program, || {
            u.flush_cache_records();
            assert_eq!(u.cache.as_ref().unwrap().stats.recorded, recorded + 1);
            let mut cache = u.cache.take().unwrap();
            let recorded = cache.stats.recorded;
            let bytes = cache.pending_bytes();
            // Simulate an O2 build finishing after O3 has installed.
            cache.record_o2(&u, id, &first.0, &first.2, &first.1);
            assert_eq!(cache.stats.recorded, recorded);
            assert_eq!(cache.pending_bytes(), bytes);
            let spill = cache.spill_take().unwrap();
            u.cache = Some(cache);
            entry = Some(u.fns[id as usize].code);
            spill
        });
        assert!(spill.write().unwrap() > 0);
    }
    let entry = entry.unwrap();
    let mut result = 0u32;
    unsafe { call_jit(up, entry, &mut result as *mut u32 as u64) }
        .unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    assert_eq!(result, 19900);
    // As in the settling fixture, process exit owns the runtime's teardown.
    std::mem::forget(u);
}

#[test]
fn settle_rebuild_pins_tier1_and_closure_history() {
    isolated_test!(settle_rebuild_pins_tier1_and_closure_history);
    // Unit installs process-global runtime state. Cache fixtures in the same harness can
    // leave a reentrant unit lock behind; run this fixture in its own runtime process.
    const CHILD: &str = "MAKEPAD_RUN_SETTLE_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::settle_rebuild_pins_tier1_and_closure_history", "--test-threads=1", "--nocapture"])
            .env(CHILD, "1").env("MAKEPAD_RUN_ICE_BT", "1").status().unwrap();
        assert!(status.success());
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    if std::env::var_os("MAKEPAD_RUN_STD").is_none() {
        std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    }
    let dir = std::env::temp_dir().join(format!("relax-settle-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::env::set_var("MAKEPAD_RUN_CACHE_DIR", dir.join("cache"));
    let path = dir.join("input.rs");
    let source = r#"
const fn width() -> usize { 3 }
const WIDTH: usize = width();
#[inline(never)]
fn apply(x: u32) -> u32 {
    let a = [x; WIDTH];
    a.iter().map(|v| *v + 7).sum::<u32>()
}
fn main() { let _ = apply(4); }
fn unrelated(x: u32) -> u32 { x + 1 }
"#.replace("let a =", &format!("{}let a =", "let _ = x; ".repeat(50)));
    std::fs::write(&path, &source).unwrap();
    let (prog, core, krate) = crate::load_with_std(&std::sync::Arc::new(crate::Session::default()), path.to_str().unwrap(), "settle_test", false).unwrap();
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == "apply").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let (id, file, first, spill) = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(FnKey::Inst(def, Vec::new()));
        let file = u.prog.def(def).file;
        let first = unsafe { compile_fn(up, id) }.unwrap();
        u.install(id, &first.0, &first.2, first.1.clone(), 0).unwrap();
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::On).unwrap();
        cache.record(&u, id, &first.0, &first.2, &first.1);
        if cache.pending_bytes() == 0 {
            use crate::types::fold::{TypeFolder, super_fold_ty};
            struct KeyAudit(std::collections::HashSet<TyId>);
            impl TypeFolder for KeyAudit {
                fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
                    if !self.0.insert(ty) { return ty; }
                    if !tys.is_concrete(ty) || matches!(tys.kind(ty), TyKind::FnPtr(..)) {
                        eprintln!("settle cache type: {ty:?} {:?}, flags {:?}, binder depth {depth}", std::mem::discriminant(tys.kind(ty)), tys.flags(ty));
                        if let TyKind::FnPtr(inputs, output, _, count) = tys.kind(ty) { eprintln!("settle cache signature: {inputs:?} -> {output:?}, {count} bound regions"); }
                    }
                    match tys.kind(ty) {
                        TyKind::Adt(_, _, rs) | TyKind::FnDef(_, _, rs) | TyKind::Alias(_, crate::types::AliasTy { regions: rs, .. }) if *rs != crate::types::RegionArgsId::EMPTY => eprintln!("settle cache region args: {ty:?} {:?}", tys.region_args(*rs)),
                        _ => {}
                    }
                    super_fold_ty(tys, ty, self, depth)
                }
                fn fold_region(&mut self, tys: &crate::types::Types, region: crate::types::RegionId, depth: u32) -> crate::types::RegionId {
                    if region != crate::types::RegionId::ERASED { eprintln!("settle cache region: {:?}, binder depth {depth}", tys.region_kind(region)); }
                    region
                }
            }
            for &(callee, _) in &u.fns[id as usize].rir.as_ref().unwrap().nounwind_inputs {
                let entry = &u.fns[callee as usize];
                let types = match &entry.key { FnKey::Inst(_, args) => args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, *t)).collect::<Vec<_>>(), _ => Vec::new() };
                eprintln!("settle cache input: {} {:?} {:?}", entry.name, entry.key, types);
                if let FnKey::Inst(_, args) = &entry.key {
                    let mut audit = KeyAudit(Default::default());
                    for &ty in args {
                        eprintln!("settle cache arg: {ty:?}, monomorphic {}, erased {}, flags {:?}", u.tcx.tys.is_concrete(ty), u.tcx.tys.erase_regions(ty) == ty, u.tcx.tys.flags(ty));
                        audit.fold_ty(&u.tcx.tys, ty, 0);
                    }
                }
            }
        }
        assert!(cache.pending_bytes() > 0, "cache rejected fixture: {:?}", cache.why_unencodable);
        let spill = cache.spill_take().unwrap();
        assert_eq!(cache.pending_bytes(), 0);
        (id, file, first, spill)
    });
    assert!(spill.write().unwrap() > 0);
    let (mut verify, rir, calls, closure, closure_rir, mut dropped, retired) = crate::owner::call(crate::owner::Class::Program, || {
        let mut verify = crate::cache::Cache::open(&u, crate::cache::Mode::Verify).unwrap();
        assert!(matches!(verify.lookup(&mut u, id), Some(_)), "verification must have a real disk entry");
        let rir = u.fns[id as usize].rir.as_ref().unwrap().dump();
        let calls = u.fns[id as usize].calls.clone();
        let definition_body = u.tcx.typeck(&u.prog, def);
        let closure = u.fns.iter().enumerate().find_map(|(i, f)| {
            matches!(f.key, FnKey::Closure(_, _, p) if p == id).then_some(i as u32)
        }).unwrap();
        unsafe { build_rir(up, closure) }.unwrap();
        let closure_rir = u.fns[closure as usize].rir.as_ref().unwrap().dump();
        let c = unsafe { compile_fn(up, closure) }.unwrap();
        u.install(closure, &c.0, &c.2, c.1, 0).unwrap();
        assert!(u.fns[id as usize].rir.is_none(), "last closure reader releases its installed owner");
        assert!(!u.has_body(id, file));
        unsafe { build_rir(up, id) }.unwrap();
        assert!(std::sync::Arc::ptr_eq(&definition_body, &u.tcx.typeck(&u.prog, def)), "settling instance storage preserves the definition query");
        let unrelated_def = u.prog.defs.iter().enumerate().find_map(|(i, d)| {
            let id = DefId(i as u32);
            (d.krate == krate && u.prog.name(id) == "unrelated").then_some(id)
        }).unwrap();
        let unrelated = u.fn_id(FnKey::Inst(unrelated_def, Vec::new()));
        let code = unsafe { compile_fn(up, unrelated) }.unwrap();
        u.install(unrelated, &code.0, &code.2, code.1, 0).unwrap();
        let saved_refs = std::mem::replace(&mut u.fns[id as usize].rir_refs, vec![unrelated].into_boxed_slice());
        // Pending and popped/in-flight O3 jobs both pin their input slots.
        TIER_QUEUE.lock().unwrap().push((id, Q_CALLS, wall_ns(), 0));
        TIER_ACTIVE.lock().unwrap().push(closure);
        let active = TierActive(closure);
        let generation = reclaim_generation();
        drop_completed_body(&mut u, id);
        drop_completed_body(&mut u, closure);
        unsafe { settle_drop(up) };
        assert!(u.fns[id as usize].rir.is_some());
        assert!(u.fns[closure as usize].rir.is_some());
        assert!(u.fns[unrelated as usize].rir.is_some(), "queued readers pin their known callee inputs too");
        u.fns[id as usize].rir_refs = saved_refs;
        TIER_QUEUE.lock().unwrap().clear();
        drop(active);
        assert!(reclaim_generation() > generation, "finishing the last reader must wake reclamation without a new install");
        // A suspended O1 front must keep all of its not-yet-snapshotted candidates.
        u.fns[id as usize].building = true;
        let saved_calls = std::mem::replace(&mut u.fns[id as usize].calls, vec![closure].into_boxed_slice());
        let checking = FRONT_RW.read().unwrap();
        let partial = unsafe { settle_drop(up) };
        assert!(partial.front_pending, "a busy ahead checker postpones only shared memo removal");
        assert!(u.fns[id as usize].rir.is_some());
        assert!(u.fns[closure as usize].rir.is_some(), "a suspended front pins its inline candidate");
        assert!(u.fns[unrelated as usize].rir.is_none(), "an unrelated completed body need not wait for that front");
        drop(checking);
        u.fns[id as usize].calls = saved_calls;
        u.fns[id as usize].building = false;
        u.fns[id as usize].inflight = true;
        unsafe { settle_drop(up) };
        assert!(u.fns[id as usize].rir.is_some());
        u.fns[id as usize].inflight = false;
        drop_completed_body(&mut u, id);
        assert!(u.fns[id as usize].rir.is_none(), "installation releases the completed slot immediately");
        assert!(!u.has_body(id, file));
        unsafe { build_rir(up, closure) }.unwrap();
        let retired = std::sync::Arc::downgrade(u.fns[closure as usize].rir.as_ref().unwrap());
        let dropped = unsafe { settle_drop(up) };
        assert!(dropped.fns >= 1);
        assert!(u.fns[id as usize].rir.is_none());
        assert!(u.fns[closure as usize].rir.is_none());
        assert!(retired.upgrade().is_some(), "unlinking must defer destruction until after unlocking");
        (verify, rir, calls, closure, closure_rir, dropped, retired)
    });
    dropped.release_rir();
    crate::owner::call(crate::owner::Class::Program, || {
        assert!(retired.upgrade().is_none(), "retired RIR must actually be released off the unit lock");
        assert_eq!(u.fns[id as usize].calls, calls);
        assert!(!u.has_body(id, file));
        // Rebuild the closure FIRST: its compiled parent no longer has a checked body or RIR.
        unsafe { build_rir(up, closure) }.unwrap();
        assert_eq!(u.fns[closure as usize].rir.as_ref().unwrap().dump(), closure_rir);
        let rebuilt = unsafe { compile_fn(up, id) }.unwrap();
        assert_eq!(u.fns[id as usize].rir.as_ref().unwrap().dump(), rir);
        assert_eq!(rebuilt.0, first.0, "O1 machine code changed after settle");
        assert!(verify.verify_entry(&u, id, &rebuilt.0, &rebuilt.2), "cache verify after settle");
        unsafe { settle_drop(up) };
    });
    assert!(unsafe { compile_tier1(up, id) }, "O3 must rebuild settled RIR");
    crate::owner::call(crate::owner::Class::Program, || {
        unsafe { settle_drop(up) };
        assert!(!u.has_body(id, file));
    });
    // Apply a real edit after eviction; old compiled closures still rebuild from old text.
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file, path: path.to_str().unwrap().to_string(), src: source.replace("*v + 7", "*v + 9").into_bytes(),
    }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
    crate::owner::call(crate::owner::Class::Program, || {
        assert!(u.has_body(id, file), "reload must preserve the old closure owner");
        u.fns[closure as usize].rir = None;
        unsafe { build_rir(up, closure) }.unwrap();
        assert_eq!(u.fns[closure as usize].rir.as_ref().unwrap().dump(), closure_rir);
        // Unit normally lives to process exit; leak it here to avoid tearing down process-global
        // JIT workers from this test. The test process releases its executable reservation.
        std::mem::forget(u);
    });
    std::fs::remove_file(path).unwrap();
    // Keep the isolated cache for inspection; it lives under the OS temporary directory.
}

#[test]
fn round23_fnptr_keys_preserve_receiver_abi() {
    isolated_test!(round23_fnptr_keys_preserve_receiver_abi);
    use relax_types::instance::ShimKind;
    let tcx = Tcx::new();
    let pointer = tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&[tcx.tys.u32_]), tcx.tys.u32_, crate::types::FnHeader::RUST, 0));
    let tuple = tcx.tys.intern(TyKind::Tuple(tcx.tys.mk_list(&[tcx.tys.u32_])));
    let key = |method| FnKey::Shim(ShimInstance { kind: ShimKind::FnPtr(DefId(method), pointer), args: vec![pointer, tuple] });
    let mut map = std::collections::HashMap::new();
    for method in 0..3 { assert!(map.insert(key(method), method).is_none()); }
    for method in 0..3 { assert_eq!(map[&key(method)], method); }
}

#[test]
fn round42_const_and_build_cancellation_are_retryable() {
    isolated_test!(round42_const_and_build_cancellation_are_retryable);
    const CHILD: &str = "MAKEPAD_RUN_CONST_PREEMPT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round42_const_and_build_cancellation_are_retryable", "--test-threads=1"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success());
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    let dir = std::env::temp_dir().join(format!("relax-preempt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, "#![no_std]\nconst fn width() -> u32 { 3 } const VALUE: u32 = width(); fn target() -> u32 { width() }").unwrap();
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(&session);
    prog.load_crate("preempt", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let named = |name| u.prog.defs.iter().enumerate().find_map(|(i, _)| (u.prog.name(DefId(i as u32)) == name).then_some(DefId(i as u32))).unwrap();
    let value = named("VALUE"); let target = named("target");
    let env = crate::tcx::TypingEnv::fully_monomorphized(&u.prog);
    u.tcx.type_of(&u.prog, value).unwrap();
    // Enter the cancellation condition only after the query has reached the native provider.
    fn cancel_provider(context: usize, env: crate::tcx::TypingEnv, def: DefId, args: &[crate::consts::GenericArg]) -> crate::const_eval::EvalToValTreeResult {
        let _writer = crate::preempt::WriterWaiting::new();
        let _p = crate::preempt::Preemptible::on();
        const_eval_to_valtree(context, env, def, args)
    }
    let original = u.tcx.const_eval_provider.unwrap();
    u.tcx.const_eval_provider.as_mut().unwrap().evaluate = cancel_provider;
    for depth in [0, 2] {
        let body = || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| u.tcx.const_eval_resolve(&u.prog, env, value, &[])));
            assert!(result.is_err_and(|p| crate::preempt::is_preempted(&*p)));
            assert!(!u.tcx.const_was_evaluated(value));
            assert!(!u.consts.contains_key(&(value, vec![])));
            assert!(!u.const_failed.contains(&(value, vec![])));
            assert_eq!(u.tcx.dcx.err_count(), 0);
            assert!(u.errors.is_empty());
        };
        // depth 0: no unit call; depth 2: two nested ones
        if depth == 0 { body() } else { crate::owner::call(crate::owner::Class::Program, || crate::owner::call(crate::owner::Class::Program, body)) }
    }
    u.tcx.const_eval_provider = Some(original);
    let value = u.tcx.const_eval_resolve(&u.prog, env, value, &[]).unwrap();
    assert_eq!(value.try_to_leaf().unwrap().to_bits(4), 3);
    crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(FnKey::Inst(target, vec![]));
        let writer = crate::preempt::WriterWaiting::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _p = crate::preempt::Preemptible::on();
            unsafe { build_rir_mode(up, id, RirMode::Tier0) }
        }));
        assert!(result.is_err_and(|p| crate::preempt::is_preempted(&*p)));
        assert!(!u.fns[id as usize].building);
        assert!(u.fns[id as usize].build_err.is_none());
        assert!(u.fns[id as usize].rir.is_none());
        assert!(BUILD_STACK.with(|s| s.borrow().is_empty()));
        drop(writer);
        unsafe { build_rir_mode(up, id, RirMode::Tier0) }.unwrap();
        assert!(u.fns[id as usize].rir.is_some());
    });
}

#[test]
fn round50_initializer_reload_policy() {
    isolated_test!(round50_initializer_reload_policy);
    const CHILD: &str = "MAKEPAD_RUN_R50_INITIALIZER_CHILD";
    let Ok(case) = std::env::var(CHILD) else {
        // (R2, lead decision 52: a foldable const read only by an ordinary fn, and a static,
        // stay on the per-file path)
        for case in ["body", "refuse", "literal", "static", "static_unused"] {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::settle_tests::round50_initializer_reload_policy", "--nocapture", "--test-threads=1"])
                .env(CHILD, case).env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0").output().unwrap();
            assert!(result.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
        }
        return;
    };
    crate::borrowck::lazy::disable_gandalf();
    let source = "#![no_std] const VALUE: u32 = 3; static TABLE: [u32; 2] = [5, 6]; static UNUSED: u32 = compute(); const fn compute() -> u32 { 4 } fn read(out: *mut u32) { unsafe { *out = VALUE; } } fn table(out: *mut u32) { unsafe { *out = TABLE[0]; } }";
    let path = std::env::temp_dir().join(format!("relax-r50-init-{}.rs", std::process::id()));
    std::fs::write(&path, source).unwrap();
    let mut session = crate::Session::default();
    // R2 cases keep the default constant evaluation: a foldable initializer is folded, never
    // executed (with CTFE off every initializer runs as code, which is T11's subject)
    if !matches!(case.as_str(), "literal" | "static" | "static_unused") { session.options.ctfe = crate::options::Ctfe::Off; }
    session.options.opt = vec!["-inline".into()];
    let session = std::sync::Arc::new(session);
    let mut prog = Program::new(&session);
    prog.load_crate("reload_init", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let other_path = path.with_extension("other.rs");
    std::fs::write(&other_path, "#![no_std] fn unrelated() { let _x = 0u32; }").unwrap();
    prog.load_crate("unrelated", other_path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    std::fs::remove_file(other_path).unwrap();
    let named = |name| prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == name).then_some(DefId(i as u32))).unwrap();
    let (value, read, unrelated, table) = (named("VALUE"), named("read"), named("unrelated"), named("table"));
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let other_before = u.tcx.typeck(&u.prog, unrelated);
    assert!(other_before.errors.is_empty(), "{:?}", other_before.errors);
    let entry = u.entry(u.fn_id(FnKey::Inst(read, vec![])));
    let mut result = 0u32;
    unsafe { call_jit(up, entry, &mut result as *mut u32 as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(result, 3);
    // TABLE's storage exists from here on (R2: its initializer is never run again)
    let table_entry = u.entry(u.fn_id(FnKey::Inst(table, vec![])));
    unsafe { call_jit(up, table_entry, &mut result as *mut u32 as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(result, 5);
    let old_addr = u.const_addr(value, vec![]).unwrap();
    let runs = u.stats.ctfe_code;
    let (candidate, expected) = match case.as_str() {
        "literal" => (source.replace("= 3;", "= 2 + 2;"), crate::reload::Rung::Patch),
        "static" => (source.replace("[5, 6]", "[7, 8]"), crate::reload::Rung::Patch),
        "static_unused" => (source.replace("UNUSED: u32 = compute();", "UNUSED: u32 = compute() + 1;"), crate::reload::Rung::Patch),
        "nonfoldable" => (source.replace("= 3;", "= compute();"), crate::reload::Rung::Restart),
        "body" => (source.replace("unsafe {", "let _x = 1u32; unsafe {"), crate::reload::Rung::Patch),
        "refuse" => (source.replace("unsafe {", "let _x: u8 = true; unsafe {"), crate::reload::Rung::Refuse),
        "inline" => (source.replace("*out = VALUE", "*out = const { compute() }"), crate::reload::Rung::Restart),
        "inline_literal" => (source.replace("*out = VALUE", "*out = const { 3u32 }"), crate::reload::Rung::Patch),
        _ => unreachable!(),
    };
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: u.prog.def(read).file, path: path.to_string_lossy().into_owned(), src: candidate.into_bytes() }]) };
    assert_eq!(outcome.rung, expected, "{:?}", outcome.lines);
    assert_eq!(u.stats.ctfe_code, runs, "transaction executed pending code");
    unsafe { call_jit(up, entry, &mut result as *mut u32 as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(result, if case == "literal" { 4 } else { 3 });
    if case == "body" || case == "refuse" {
        assert_eq!(u.const_addr(value, vec![]), Some(old_addr), "ordinary body edit invalidated constant storage");
    }
    if matches!(case.as_str(), "body" | "refuse" | "literal" | "static") {
        assert!(std::sync::Arc::ptr_eq(&other_before, &u.tcx.typeck(&u.prog, unrelated)), "other-file typeck recomputed");
    }
    if case == "static_unused" {
        // no storage, not literal, no scalar fold: the round-51 CTFE-relevant path (full publication)
        assert!(!std::sync::Arc::ptr_eq(&other_before, &u.tcx.typeck(&u.prog, unrelated)), "an unfoldable static without storage took the per-file path");
    }
    if let Some(undo) = outcome.undo { unsafe { crate::reload::rollback(up, undo); } }
    unsafe { call_jit(up, entry, &mut result as *mut u32 as u64).unwrap_or_else(|p| panic!("{}", p.message)); }
    assert_eq!(result, 3);
    assert_eq!(u.const_addr(value, vec![]), Some(old_addr));
    if matches!(case.as_str(), "body" | "refuse" | "literal" | "static") {
        assert!(std::sync::Arc::ptr_eq(&other_before, &u.tcx.typeck(&u.prog, unrelated)), "rollback recomputed other-file typeck");
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn round50_opaque_body_invalidates_solver_answer() {
    isolated_test!(round50_opaque_body_invalidates_solver_answer);
    const CHILD: &str = "MAKEPAD_RUN_R50_OPAQUE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round50_opaque_body_invalidates_solver_answer", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0").output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    let source = "#![no_std] trait Marker {} impl Marker for u8 {} impl Marker for u32 {} fn owner() -> impl Marker { 1u8 }";
    let path = std::env::temp_dir().join(format!("relax-r50-opaque-{}.rs", std::process::id()));
    std::fs::write(&path, source).unwrap();
    let session = std::sync::Arc::new(crate::Session::default());
    let mut prog = Program::new(&session);
    prog.load_crate("reload_opaque", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let owner = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "owner").then_some(DefId(i as u32))).unwrap();
    let opaque = prog.opaque_types_defined_by[&owner][0];
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let hidden = |u: &Unit| {
        let alias = u.tcx.alias_ty_with_args(crate::types::AliasTyKind::Opaque { def_id: opaque }, &[]);
        u.tcx.try_normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), alias).unwrap()
    };
    assert_eq!(hidden(&u), u.tcx.tys.u8_);
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: u.prog.def(owner).file, path: path.to_string_lossy().into_owned(), src: source.replace("1u8", "2u32").into_bytes() }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
    assert_eq!(hidden(&u), u.tcx.tys.u32_);
    unsafe { crate::reload::rollback(up, outcome.undo.unwrap()); }
    assert_eq!(hidden(&u), u.tcx.tys.u8_);
    std::fs::remove_file(path).unwrap();
}

// T11: reload evaluates candidates with running native code; owner: codegen2.
#[test]
#[ignore = "T11: candidate CTFE must be transaction-staged or restart; codegen2"]
fn round50_foldable_const_with_native_dependent() {
    isolated_test!(round50_foldable_const_with_native_dependent);
    const CHILD: &str = "MAKEPAD_RUN_R50_FOLDED_DEPENDENT_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "jit::settle_tests::round50_foldable_const_with_native_dependent", "--ignored", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0").output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    let source = "#![no_std] const BASE: usize = 3; const fn width() -> usize { BASE } const VALUE: usize = width(); fn probe() { let _: [u8; VALUE] = [0; 3]; }";
    let path = std::env::temp_dir().join(format!("relax-r50-folded-dependent-{}.rs", std::process::id()));
    std::fs::write(&path, source).unwrap();
    let mut session = crate::Session::default();
    session.options.ctfe = crate::options::Ctfe::Off;
    session.options.opt = vec!["-inline".into()];
    let session = std::sync::Arc::new(session);
    let mut prog = Program::new(&session);
    prog.load_crate("folded_dependent", path.to_str().unwrap(), 2021, crate::cfg::CfgSet::host(&session, &[]), vec![]).unwrap();
    let probe = prog.defs.iter().enumerate().find_map(|(i, _)| (prog.name(DefId(i as u32)) == "probe").then_some(DefId(i as u32))).unwrap();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, 0);
    let up = &mut *u as *mut Unit;
    let before = u.tcx.typeck(&u.prog, probe);
    assert!(before.errors.is_empty(), "{:?}", before.errors);
    let runs = u.stats.ctfe_code;
    assert!(runs > 0);
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: u.prog.def(probe).file, path: path.to_string_lossy().into_owned(), src: source.replace("= 3;", "= 4;").replace("[0; 3]", "[0; 4]").into_bytes() }]) };
    eprintln!("foldable dependency: {:?}: {:?}; native executions {} -> {}", outcome.rung, outcome.lines, runs, u.stats.ctfe_code);
    assert_eq!(outcome.rung, crate::reload::Rung::Restart, "RELOAD' must protect the type sized by the native dependent: {:?}", outcome.lines);
    assert_eq!(u.stats.ctfe_code, runs, "pending initializer executed during reload");
    assert!(u.tcx.typeck(&u.prog, probe).errors.is_empty());
    std::fs::remove_file(path).unwrap();
}

// T11: new inline constants reach native CTFE during the reload transaction.
// Owner: codegen2; retained separately so ordinary body checks remain enabled.
#[test]
#[ignore = "T11: inline-const candidate CTFE must be transaction-staged or restart; codegen2"]
fn round50_inline_const_reload() {
    isolated_test!(round50_inline_const_reload);
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "jit::settle_tests::round50_initializer_reload_policy", "--nocapture", "--test-threads=1"])
        .env("MAKEPAD_RUN_R50_INITIALIZER_CHILD", "inline_literal").env("MAKEPAD_RUN_CACHE", "off").env("MAKEPAD_RUN_GANDALF", "0").output().unwrap();
    assert!(result.status.success(), "{}\n{}", String::from_utf8_lossy(&result.stdout), String::from_utf8_lossy(&result.stderr));
}

#[test]
fn deferred_cache_record_is_dropped_when_a_source_changed() {
    isolated_test!(deferred_cache_record_is_dropped_when_a_source_changed);
    // A deferred record (Cache::defer_record) is encoded after the install; an edit, rollback or
    // reload in between (the patch epoch moves) must drop it, never key old code by new text.
    // Control: the same record with no change in between is encoded.
    const CHILD: &str = "MAKEPAD_RUN_DEFERRED_RECORD_TEST";
    const DIR: &str = "MAKEPAD_RUN_DEFERRED_RECORD_TEST_DIR";
    let Ok(phase) = std::env::var(CHILD) else {
        let dir = std::env::temp_dir().join(format!("relax-deferred-record-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("input.rs"), "pub fn hot(out: &mut u32) { *out = 7; }\nfn main() {}\n").unwrap();
        for phase in ["current", "stale"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "jit::settle_tests::deferred_cache_record_is_dropped_when_a_source_changed", "--nocapture", "--test-threads=1"])
                .env(CHILD, phase).env(DIR, dir.join(phase)).env("MAKEPAD_RUN_SPEC", "0")
                .output().unwrap();
            assert!(output.status.success(), "{phase}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    crate::borrowck::lazy::disable_gandalf();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::path::PathBuf::from(std::env::var_os(DIR).unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.parent().unwrap().join("input.rs");
    std::env::set_var("MAKEPAD_RUN_CACHE_DIR", dir.join("cache"));
    let session = std::sync::Arc::new(crate::Session::default());
    let (prog, core, krate) = crate::load_with_std(&session, src.to_str().unwrap(), "deferred_record", false).unwrap();
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && prog.name(id) == "hot").then_some(id)
    }).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    let mut u = Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(FnKey::Inst(def, Vec::new()));
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        let built = unsafe { compile_fn(up, id) }.unwrap();
        let mut cache = u.cache.take().unwrap();
        let pending = cache.defer_record(&u, id, &built.0, &built.2, &built.1).expect("a fresh install is deferrable");
        if phase == "stale" {
            bump_patch_epoch();
        }
        cache.record_pending(&u, &pending);
        match phase.as_str() {
            "current" => assert!(cache.pending_bytes() > 0 && cache.stats.recorded == 1, "the control record must be encoded"),
            _ => assert!(cache.pending_bytes() == 0 && cache.stats.recorded == 0, "a record from before an edit must not be encoded"),
        }
        u.cache = Some(cache);
    });
    std::mem::forget(u);
}
