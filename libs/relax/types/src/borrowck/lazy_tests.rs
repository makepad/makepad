use super::*;
use super::super::event::{admit, app_crate, set_app_scope};
use std::sync::Barrier;
use std::time::Duration;

const SOURCE: &str = "#[lang = \"sized\"] trait Sized {} #[lang = \"copy\"] trait Copy {} impl Copy for i32 {} impl Copy for bool {} fn a(x: &mut i32) { let p = &mut *x; let q = &mut *x; *p = 1; *q = 2; }\nfn b(x: &mut i32) { let p = &mut *x; let q = &mut *x; *p = 1; *q = 2; }\nfn c(x: &mut i32) { let p = &mut *x; let q = &mut *x; *p = 1; *q = 2; }\nfn clean() {}\nfn bad(x: &mut i32) { let p = &*x; let q = &mut *x; let z = *p; *q = z; }\nfn typefail() { let n: i32 = false; }\n";

// The worker and event bus are process-wide, as in the compiler's settle harness.
fn child(name: &str) -> bool {
    if std::env::var_os("GANDALF_TEST_CHILD").is_some() { return false; }
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("borrowck::lazy::lazy_tests::{name}"), "--test-threads=1"])
        .env("GANDALF_TEST_CHILD", "1").status().unwrap();
    assert!(status.success()); true
}
fn fixture(dir: &str, source: &str) -> (Program, Tcx) {
    let session = Arc::new(crate::Session::default());
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gandalf_fixture").join(dir);
    let root = directory.join(format!("input-{}.rs", std::process::id())).display().to_string();
    std::fs::write(&root, source).unwrap();
    let mut prog = Program::new(&session);
    prog.load_crate("fixture", &root, 2021, crate::cfg::CfgSet::host(&session, &[]), Vec::new()).unwrap();
    std::fs::remove_file(&root).unwrap();
    prog.resolve_imports();
    let mut tcx = Tcx::new(); tcx.collect(&prog);
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    set_app_scope(vec![app_root()]);
    (prog, tcx)
}
/// The app fixture crate's root source path: the one crate the tests' scope admits.
fn app_root() -> String {
    format!("{}/tests/gandalf_fixture/app/input-{}.rs", env!("CARGO_MANIFEST_DIR"), std::process::id())
}
fn named(prog: &Program, name: &str) -> DefId {
    (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|d| prog.name(*d) == name).unwrap()
}
struct TestFront {
    prog: Program, tcx: Tcx, quiet: AtomicU64, calls: AtomicU64,
    block_call: u64, entered: Barrier, release: Barrier,
    /// the front end's lock: checks read under it, a live patch writes (lazy::carry_epoch)
    lock: std::sync::RwLock<()>,
}
impl TestFront {
    fn new(block_call: u64) -> Arc<Self> {
        let (prog, tcx) = fixture("app", SOURCE);
        Arc::new(Self { prog, tcx, quiet: AtomicU64::new(0), calls: AtomicU64::new(0), block_call,
            entered: Barrier::new(2), release: Barrier::new(2), lock: std::sync::RwLock::new(()) })
    }
    /// A live patch: the next collect epoch, with `keep`'s owners carried, under the front end's
    /// write lock as the driver does it (no check runs meanwhile, none sees the new epoch early).
    fn patch(&self, keep: &dyn Fn(DefId) -> bool) {
        let _w = self.lock.write().unwrap();
        let from = self.tcx.collect_epoch.fetch_add(1, Ordering::SeqCst);
        carry_epoch(self.identity(), from, from + 1, keep);
    }
    fn epoch(&self) -> u64 { self.tcx.collect_epoch.load(Ordering::SeqCst) }
    fn enqueue(self: &Arc<Self>, name: &str, cached_clean: bool) -> DefId {
        let def = named(&self.prog, name);
        let src: Arc<dyn FrontSource> = self.clone();
        note_compiled(&self.prog.session.options, &src, NoteFn { def, args: Vec::new(), cache_key: None, epoch: self.epoch(), rank: None, cached: cached_clean.then_some(StoredVerdict::Clean) });
        def
    }
    fn ungate(&self) { self.quiet.store(u64::MAX, Ordering::SeqCst); WAKE.notify_one(); }
}
impl FrontSource for TestFront {
    fn with_front(&self, f: &mut dyn FnMut(&Program, &Tcx, &crate::tcx::EvalCx<'_>)) -> bool {
        let _r = self.lock.read().unwrap();
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == self.block_call { self.entered.wait(); self.release.wait(); }
        let state = crate::tcx::EvalState::new();
        f(&self.prog, &self.tcx, &crate::tcx::EvalCx::new(&self.prog, &self.tcx, crate::tcx::TypingEnv::fully_monomorphized(&self.prog), &state));
        true
    }
    fn quiet_ns(&self) -> u64 { self.quiet.load(Ordering::SeqCst) }
    fn lower_priority(&self) {}
    fn identity(&self) -> usize { self as *const Self as usize }
}
// The waits below have no wall-clock limit: the worker's progress depends on the machine (a
// JIT-run test binary compiles the checker on first use, and a loaded machine runs it slower);
// a worker that never delivers hangs the test, which the harness reports.
fn event(rx: &Receiver<GandalfEvent>) -> GandalfEvent { rx.recv().expect("per-function event") }
fn wait_verdict(f: &TestFront, def: DefId) -> FnVerdict {
    loop {
        let v = verdict(f.identity(), def, &[], f.epoch());
        if v != FnVerdict::Pending { return v; }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn events_arrive_per_fn_before_queue_drains() {
    if child("events_arrive_per_fn_before_queue_drains") { return; }
    let front = TestFront::new(4);
    let (id, rx) = subscribe();
    for name in ["a", "b", "c", "clean"] { front.enqueue(name, false); }
    front.ungate();
    for (i, name) in ["a", "b", "c"].iter().enumerate() {
        let ev = event(&rx);
        let def = named(&front.prog, name);
        assert_eq!(ev.error.fn_path, front.prog.def_path(def));
        assert_eq!(ev.error.file, front.prog.file_paths[front.prog.def(def).file as usize]);
        assert_eq!((ev.error.line, ev.error.col, ev.error.code), (i as u32 + 1, if i == 0 { 153 } else { 46 }, "E0499"));
        assert_eq!(ev.origin, Origin::Background);
    }
    front.entered.wait();
    assert_eq!(verdict(front.identity(), named(&front.prog, "clean"), &[], front.epoch()), FnVerdict::Pending);
    front.release.wait();
    assert_eq!(wait_verdict(&front, named(&front.prog, "clean")), FnVerdict::Clean);
    unsubscribe(id); assert!(rx.recv().is_err()); stop();
}

#[test]
fn priority_order_respected() {
    if child("priority_order_respected") { return; }
    let front = TestFront::new(0);
    let (id, rx) = subscribe();
    let a = front.enqueue("a", false); let b = front.enqueue("b", false); let c = front.enqueue("c", false);
    let epoch = front.epoch();
    let stale = STALE.load(Ordering::Relaxed);
    set_ranks(front.identity(), epoch + 1, &[(c, vec![], 0), (a, vec![], 5)]);
    assert_eq!(STALE.load(Ordering::Relaxed) - stale, 3);
    for def in [a, b, c] { assert_eq!(verdict(front.identity(), def, &[], epoch), FnVerdict::Unchecked("stale".into())); }
    set_ranks(front.identity(), epoch, &[(b, vec![], 0)]);
    front.tcx.collect_epoch.store(epoch + 1, Ordering::SeqCst);
    for name in ["a", "b", "c"] { front.enqueue(name, false); }
    set_ranks(front.identity(), epoch + 1, &[(a, vec![], 5)]);
    front.ungate();
    let events = [event(&rx), event(&rx), event(&rx)];
    assert_eq!(events.each_ref().map(|e| e.error.owner), [c, a, b]);
    assert!(events.windows(2).all(|p| p[0].seq < p[1].seq));
    unsubscribe(id); stop();
}

#[test]
fn gate_returns_app_errors() {
    if child("gate_returns_app_errors") { return; }
    let (prog, tcx) = fixture("app", SOURCE);
    let (id, rx) = subscribe(); let bad = named(&prog, "bad");
    let epoch = tcx.collect_epoch.load(Ordering::SeqCst);
    let g = check_now(&prog, &tcx, &CheckRequest { changed: &[bad], instances: &[], checked: &[], growth: None, origin: Origin::Gate, epoch, fv: None });
    assert!(g.refuses()); assert_eq!(g.events.len(), 1); assert_eq!(g.events[0].error.code, "E0502");
    assert_eq!(event(&rx), g.events[0]); assert_eq!(g.events[0].origin, Origin::Gate);
    let clean = named(&prog, "clean");
    let g = check_now(&prog, &tcx, &CheckRequest { changed: &[clean], instances: &[], checked: &[], growth: None, origin: Origin::Gate, epoch, fv: None });
    assert!(!g.refuses()); assert_eq!(verdict(0, clean, &[], epoch), FnVerdict::Clean);
    unsubscribe(id);
}

#[test]
fn guard_filters_non_app() {
    if child("guard_filters_non_app") { return; }
    assert!(!reject_trap_allowed());
    let (id, rx) = subscribe();
    for dir in ["framework", "vendor/dep"] {
        let (prog, tcx) = fixture(dir, SOURCE); let bad = named(&prog, "bad");
        let epoch = tcx.collect_epoch.load(Ordering::SeqCst);
        let g = check_now(&prog, &tcx, &CheckRequest { changed: &[bad], instances: &[], checked: &[], growth: None, origin: Origin::Gate, epoch, fv: None });
        assert!(!g.refuses()); assert_eq!(g.withheld, 1); assert!(rx.try_recv().is_err());
        assert!(matches!(verdict(0, bad, &[], epoch), FnVerdict::Errors(_)));
    }
    let (prog, _) = fixture("app", SOURCE);
    assert!(app_crate(&prog, 0));
    let (prog, _) = fixture("framework", SOURCE);
    assert!(!app_crate(&prog, 0));
    // Membership is by the crate's root path: its directory or the project containing it
    // does not admit it; another spelling of the same root path does.
    let project = format!("{}/tests/gandalf_fixture", env!("CARGO_MANIFEST_DIR"));
    set_app_scope(vec![format!("{project}/framework"), project.clone()]);
    assert!(!app_crate(&prog, 0));
    set_app_scope(vec![format!("{project}/app/../framework/input-{}.rs", std::process::id())]);
    assert!(app_crate(&prog, 0));
    set_app_scope(Vec::new());
    assert!(!app_crate(&prog, 0));
    // The one all-crates switch applies on top of the compiled default.
    assert!(!admit(&prog, 0));
    std::env::set_var("MAKEPAD_RUN_GANDALF_ALL", "1");
    assert!(admit(&prog, 0));
    std::env::set_var("MAKEPAD_RUN_GANDALF_ALL", "0");
    assert!(!admit(&prog, 0));
    std::env::remove_var("MAKEPAD_RUN_GANDALF_ALL");
    unsubscribe(id);
}

#[test]
fn check_before_run_decides_before_the_background() {
    if child("check_before_run_decides_before_the_background") { return; }
    // the worker stays gated (never quiet): every verdict below comes from check_before_run
    let front = TestFront::new(0);
    let src: Arc<dyn FrontSource> = front.clone();
    let (id, rx) = subscribe();
    let bad = front.enqueue("bad", false);
    assert_eq!(verdict(front.identity(), bad, &[], front.epoch()), FnVerdict::Pending);
    let refused = check_before_run(&front.prog, &front.tcx, &src, bad, front.epoch(), None, None);
    assert_eq!(refused.len(), 1); assert_eq!(refused[0].error.code, "E0502"); assert_eq!(refused[0].origin, Origin::Run);
    assert_eq!(event(&rx), refused[0]);
    assert!(matches!(verdict(front.identity(), bad, &[], front.epoch()), FnVerdict::Errors(_)));
    // asked again (another instance of the same owner): the recorded errors, no second check
    assert_eq!(check_before_run(&front.prog, &front.tcx, &src, bad, front.epoch(), None, None), refused);
    assert!(rx.try_recv().is_err());
    let clean = named(&front.prog, "clean");
    assert!(check_before_run(&front.prog, &front.tcx, &src, clean, front.epoch(), None, None).is_empty());
    assert_eq!(verdict(front.identity(), clean, &[], front.epoch()), FnVerdict::Clean);
    // a body the checker cannot certify (type errors) runs: rustc's verdict on it is typeck's
    let fail = named(&front.prog, "typefail");
    assert!(check_before_run(&front.prog, &front.tcx, &src, fail, front.epoch(), None, None).is_empty());
    // a stored clean verdict is trusted without a check
    let a = named(&front.prog, "a");
    assert!(check_before_run(&front.prog, &front.tcx, &src, a, front.epoch(), None, Some(StoredVerdict::Clean)).is_empty());
    assert_eq!(verdict(front.identity(), a, &[], front.epoch()), FnVerdict::Clean);
    unsubscribe(id); stop();
}

#[test]
fn verdict_states() {
    if child("verdict_states") { return; }
    let front = TestFront::new(0);
    let clean = front.enqueue("clean", false); let bad = front.enqueue("a", false); let fail = front.enqueue("typefail", false);
    assert_eq!(verdict(front.identity(), clean, &[], front.epoch()), FnVerdict::Pending);
    let cached = front.enqueue("b", true);
    assert_eq!(verdict(front.identity(), cached, &[], front.epoch()), FnVerdict::Clean);
    front.ungate();
    assert_eq!(wait_verdict(&front, clean), FnVerdict::Clean);
    assert!(matches!(wait_verdict(&front, bad), FnVerdict::Errors(_)));
    assert_eq!(wait_verdict(&front, fail), FnVerdict::Unchecked("typeck".into())); stop();
}

#[test]
fn render_matches_old() {
    if child("render_matches_old") { return; }
    let (prog, _) = fixture("app", "fn clean() {}\n"); let def = named(&prog, "clean");
    let span = crate::mir::Span { file: prog.def(def).file, lo: 3, hi: 8 };
    let mut d = super::super::diagnostics::Diag::new("E0502", span, "cannot borrow `x`");
    d.span_label(span, "primary"); d.note("a note"); d.help("a help");
    let e = super::super::diagnostics::capture(&prog, def, &d);
    assert_eq!(e.render(), format!("error[E0502]: cannot borrow `x`\n  --> {}:1:4\n  |\n1 | fn clean() {{}}\n  |    ^^^^^ primary\n  |\n  = note: a note\n  = help: a help\n", e.file));
    let json = format!("{{\"severity\":\"error\",\"code\":\"E0502\",\"message\":\"cannot borrow `x`\",\"file\":{},\"line\":1,\"col\":4,\"end_line\":1,\"end_col\":9,\"phase\":\"borrowck\",\"notes\":[\"a note\",\"primary\"],\"suggestion\":{{\"message\":\"a help\",\"replacement\":null}},\"item\":{}}}", crate::diag::quote(&e.file), crate::diag::quote(&e.fn_path));
    assert_eq!(e.to_diagnostic().to_json(), json);
    assert_eq!(e.one_line(), format!("{}:1:4: error[E0502]: cannot borrow `x`", e.file));
    assert!(e.mentions("x")); assert!(!e.mentions("y"));
}

#[test]
fn discovery_releases_front_between_definitions_and_checks_all_owners() {
    if child("discovery_releases_front_between_definitions_and_checks_all_owners") { return; }
    let front = TestFront::new(2);
    let src: Arc<dyn FrontSource> = front.clone();
    let (subscription, rx) = subscribe();
    note_program(&front.prog.session.options, &src, front.epoch());
    front.ungate();
    front.entered.wait();
    // At the second front acquisition, discovery has examined exactly one definition.
    // The old whole-program critical section would already have queued all six owners.
    let owners = QUEUE.lock().unwrap().as_ref().unwrap().seen.keys()
        .filter(|key| key.0 == front.identity() && key.1 != crate::program::NO_DEF).count();
    assert!(owners <= 1, "discovery held the front across {owners} owners");
    front.release.wait();
    let mut checked: Vec<_> = (0..4).map(|_| event(&rx).error.owner).collect();
    checked.sort_unstable_by_key(|d| d.0);
    let mut expected: Vec<_> = ["a", "b", "c", "bad"].iter().map(|name| named(&front.prog, name)).collect();
    expected.sort_unstable_by_key(|d| d.0);
    assert_eq!(checked, expected);
    let clean = named(&front.prog, "clean");
    while verdict(front.identity(), clean, &[], front.epoch()) != FnVerdict::Clean {
        std::thread::sleep(Duration::from_millis(5));
    }
    unsubscribe(subscription);
    stop();
}

#[test]
fn live_epoch_carries_verdicts_of_unreached_owners() {
    if child("live_epoch_carries_verdicts_of_unreached_owners") { return; }
    let front = TestFront::new(0);
    let clean = front.enqueue("clean", false); let edited = front.enqueue("a", false); let fail = front.enqueue("typefail", false);
    front.ungate();
    assert_eq!(wait_verdict(&front, clean), FnVerdict::Clean);
    assert!(matches!(wait_verdict(&front, edited), FnVerdict::Errors(_)));
    assert_eq!(wait_verdict(&front, fail), FnVerdict::Unchecked("typeck".into()));
    let checked0 = CHECKED.load(Ordering::SeqCst);
    // queued (or already running) when the patch lands: its check counts at the new epoch
    let queued = front.enqueue("c", false);
    // a live patch of `a`: every other owner reads what it read before
    front.patch(&|d| d != edited);
    assert_eq!(verdict(front.identity(), clean, &[], front.epoch()), FnVerdict::Clean);
    assert_eq!(verdict(front.identity(), fail, &[], front.epoch()), FnVerdict::Unchecked("typeck".into()));
    assert_eq!(verdict(front.identity(), edited, &[], front.epoch()), FnVerdict::Unchecked("not queued".into()));
    // noted again at the new epoch: the carried owner is not checked again, the edited one is
    front.enqueue("clean", false);
    assert_eq!(verdict(front.identity(), clean, &[], front.epoch()), FnVerdict::Clean);
    front.enqueue("a", false);
    assert!(matches!(wait_verdict(&front, edited), FnVerdict::Errors(_)));
    assert!(matches!(wait_verdict(&front, queued), FnVerdict::Errors(_)));
    assert_eq!(CHECKED.load(Ordering::SeqCst) - checked0, 2, "the carried clean owner was checked again");
    stop();
}

#[test]
fn round42_cancelled_gandalf_check_has_no_persisted_verdict() {
    if child("round42_cancelled_gandalf_check_has_no_persisted_verdict") { return; }
    let front = TestFront::new(1);
    let def = named(&front.prog, "clean");
    let src: Arc<dyn FrontSource> = front.clone();
    note_compiled(&front.prog.session.options, &src, NoteFn { def, args: Vec::new(), cache_key: Some(42), epoch: front.epoch(), rank: None, cached: None });
    front.ungate();
    front.entered.wait();
    let writer = crate::preempt::WriterWaiting::new();
    front.release.wait();
    while PREEMPTED.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(verdict(front.identity(), def, &[], front.epoch()), FnVerdict::Pending);
    assert!(take_verdict_keys().is_empty());
    assert_eq!(CHECKED.load(Ordering::Relaxed), 0);
    drop(writer);
    assert_eq!(wait_verdict(&front, def), FnVerdict::Clean);
    assert!(take_verdict_keys().contains(&(42, StoredVerdict::Clean)));
    stop();
}
