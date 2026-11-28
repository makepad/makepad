use super::*;
use crate::borrowck::lazy::StoredVerdict;
use crate::cache::{Cache, Mode};
use std::sync::{Arc, Barrier};

const SOURCE: &str = r#"
#![no_std]
#[inline(never)]
pub fn callee(a: &mut u32, b: &mut u32) {
    *a = *a + *b;
}
pub fn caller(a: &mut u32, b: &mut u32) {
    let _f = |x: &mut u32, y: &mut u32| callee(x, y);
    callee(a, b);
}
"#;

fn with_cache(tag: &str, test: impl FnOnce(&mut Unit, DefId, u32, u32)) {
    CHECK_FIRST.store(false, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("relax-cache-race-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lib.rs");
    std::fs::write(&path, SOURCE).unwrap();
    let path = path.to_str().unwrap();
    crate::borrowck::event::set_app_scope(vec![path.to_owned()]);

    let sess = Arc::new(crate::Session::default());
    let mut prog = Program::new(&sess);
    let krate = prog.load_crate("cache_race", path, 2021, crate::cfg::CfgSet::host(&sess, &[]), vec![]).unwrap();
    let def = |name: &str| (0..prog.defs.len() as u32).map(DefId).find(|&d| {
        prog.def(d).krate == krate && prog.def(d).kind == DefKind::Fn && prog.name(d) == name
    }).unwrap();
    let (caller, callee) = (def("caller"), def("callee"));
    let mut tcx = Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let mut u = Unit::new(prog, tcx, krate);
    unsafe { UNIT = &mut *u };
    enable_pipelining(&u.prog.session.options);
    let (caller_id, closure_id) = owner::call(owner::Class::Program, || {
        let caller_id = u.fn_id(FnKey::Inst(caller, vec![]));
        u.fn_id(FnKey::Inst(callee, vec![]));
        let body = crate::typeck::try_codegen_body(&u.prog, &u.tcx, caller, &[]).expect("caller body");
        assert!(body.errors.is_empty(), "{:?}", body.errors);
        let ast = &u.prog.files[body.file as usize].ast;
        let closure = (0..body.expr_ty.len()).map(|i| body.expr_at(i)).find(|&e| {
            ast.has_expr(e) && matches!(ast.expr(e).kind, crate::ast::ExprKind::Closure(_))
        }).expect("closure in caller");
        let closure_id = u.fn_id(FnKey::Closure(body.file, closure.0, caller_id));

        let root = dir.join("store");
        let epoch = u.tcx.collect_epoch.load(Ordering::SeqCst);
        let mut seed = Cache::open_in(&u, Mode::On, root.clone()).unwrap();
        let key = seed.owner_verdicts(epoch).lookup(&u.prog, caller, epoch).0.expect("owner verdict key");
        seed.record_borrowck(key, StoredVerdict::Clean);
        seed.flush().unwrap();
        drop(seed);
        let cache = Cache::open_in(&u, Mode::On, root).unwrap();
        assert_eq!(cache.owner_verdicts(epoch).lookup(&u.prog, caller, epoch).1, Some(StoredVerdict::Clean));
        u.set_cache(cache);
        (caller_id, closure_id)
    });
    test(&mut u, caller, caller_id, closure_id);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

#[derive(Clone)]
struct LookupBarriers {
    at_lookup: Arc<Barrier>,
    cache_out: Arc<Barrier>,
    reader_done: Arc<Barrier>,
    hook_entered: Arc<AtomicBool>,
}

struct FinishReader<'a>(&'a LookupBarriers);

impl Drop for FinishReader<'_> {
    fn drop(&mut self) {
        // Release the coordinator even if lowering bails or unwinds before the lookup.
        if !self.0.hook_entered.load(Ordering::SeqCst) {
            self.0.at_lookup.wait();
            self.0.cache_out.wait();
        }
        self.0.reader_done.wait();
    }
}

fn lower_with_cache_taken(u: &mut Unit, target: u32) {
    let front = u.front.clone();
    let sync = LookupBarriers {
        at_lookup: Arc::new(Barrier::new(2)),
        cache_out: Arc::new(Barrier::new(2)),
        reader_done: Arc::new(Barrier::new(2)),
        hook_entered: Arc::new(AtomicBool::new(false)),
    };
    let hook_sync = sync.clone();
    *STORED_OWNER_VERDICT_HOOK.lock().unwrap() = Some(Arc::new(move || {
        if !hook_sync.hook_entered.swap(true, Ordering::SeqCst) {
            hook_sync.at_lookup.wait();
            hook_sync.cache_out.wait();
        }
    }));
    let reader_sync = sync.clone();
    let reader_front = front.clone();
    let reader = std::thread::spawn(move || {
        let _finish = FinishReader(&reader_sync);
        assert!(!owner::is_owner());
        prefront_slot(&reader_front, target)
    });
    // The closure path asks the owner for its parent's body before it reaches the hook.
    sync.at_lookup.wait();
    owner::call(owner::Class::Program, || {
        let taken = u.cache.take();
        assert!(u.cache.is_none());
        sync.cache_out.wait();
        sync.reader_done.wait();
        u.cache = taken;
    });
    let made = reader.join();
    *STORED_OWNER_VERDICT_HOOK.lock().unwrap() = None;
    assert!(sync.hook_entered.load(Ordering::SeqCst), "reader missed the stored owner lookup");
    assert!(made.unwrap(), "reader failed to lower the slot");
    let lowered = front.lowered.lock().unwrap();
    let Some(LoweredState::Ready(l)) = lowered.get(&target) else { panic!("expected a ready lowering") };
    assert!(!l.func.plain_calls, "the stored clean verdict must select verified lowering");
    assert!(!l.callees.iter().any(|k| matches!(k, FnKey::Plain(_))), "verified lowering called a plain twin");
}

#[test]
fn off_owner_lower_ahead_does_not_race_owner_cache_take() {
    isolated_test!(off_owner_lower_ahead_does_not_race_owner_cache_take);
    with_cache("caller", |u, _, caller, _| lower_with_cache_taken(u, caller));
}

#[test]
fn off_owner_prefront_closure_does_not_race_owner_cache_take() {
    isolated_test!(off_owner_prefront_closure_does_not_race_owner_cache_take);
    with_cache("closure", |u, _, _, closure| lower_with_cache_taken(u, closure));
}

#[test]
fn stored_owner_verdict_absent_epoch_returns_none_without_poisoning() {
    isolated_test!(stored_owner_verdict_absent_epoch_returns_none_without_poisoning);
    with_cache("unpublished", |u, caller, _, _| {
        let lookup = |front: &FrontShared, epoch| {
            assert!(!owner::is_owner());
            let r = front_read("stored owner verdict test");
            r.unit_of(front).stored_owner_verdict(caller, epoch)
        };
        let epoch = owner::call(owner::Class::Program, || {
            let epoch = u.tcx.collect_epoch.fetch_add(1, Ordering::SeqCst) + 1;
            let taken = u.cache.take();
            let front = u.front.clone();
            let first = std::thread::scope(|s| s.spawn(|| lookup(&front, epoch)).join());
            u.cache = taken;
            assert_eq!(first.unwrap(), None, "an unpublished epoch has no verdict");
            // With the cache back, a reader still sees only published verdicts.
            let present = std::thread::scope(|s| s.spawn(|| lookup(&front, epoch)).join());
            assert_eq!(present.unwrap(), None, "an unpublished epoch must not fall back to u.cache");
            assert!(!front.owner_verdicts.contains_key(&epoch), "reader published an epoch");
            assert!(!front.stored_owner.contains_key(&(epoch, caller)), "reader memoized an unpublished epoch's miss");
            u.publish_owner_verdicts();
            epoch
        });
        assert_eq!(lookup(&u.front, epoch), Some(StoredVerdict::Clean), "published epoch must see the stored verdict");
    });
}

#[test]
fn owner_verdict_clean_survives_cache_take_and_epoch_bump() {
    isolated_test!(owner_verdict_clean_survives_cache_take_and_epoch_bump);
    with_cache("published", |u, _, caller, _| {
        owner::call(owner::Class::Program, || {
            u.tcx.collect_epoch.fetch_add(1, Ordering::SeqCst);
            u.publish_owner_verdicts();
            let taken = u.cache.take();
            let clean = u.verdict_clean(caller);
            u.cache = taken;
            assert!(clean, "taking the cache lost the published clean verdict");
        });
    });
}

