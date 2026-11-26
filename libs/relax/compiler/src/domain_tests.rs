use super::*;
use super::load_step_gate::{hold_load_steps, release_load_step, wait_for_loaders, wait_load_step_held, STEPS_BEGUN};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// These tests exercise the real JIT allocation and Relax std thread/TLS entry paths. The
// fixture returns from main while a detached child still owns its domain, including TLS drops.
const FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
struct TlsDrop(usize);
impl Drop for TlsDrop {
    fn drop(&mut self) {
        assert_eq!(unsafe { TOUCHED }, 42);
        Box::leak(Box::new([11u8; 8192]));
        let a = unsafe { &*(self.0 as *const [AtomicU64; 4]) };
        a[2].store(1, Ordering::SeqCst);
    }
}
thread_local! { static DROP: std::cell::RefCell<Option<TlsDrop>> = std::cell::RefCell::new(None); }
static mut TOUCHED: u64 = 0;
fn fixture(p: *mut u64) {
    unsafe { TOUCHED = 42; }
    let mut v = Vec::new();
    v.resize(1024 * 1024, 37u8);
    assert_eq!(v[100], 37);
    Box::leak(v.into_boxed_slice());
    let address = p as usize;
    std::thread::Builder::new().name("fixture-worker".into()).spawn(move || {
        let a = unsafe { &*(address as *const [AtomicU64; 4]) };
        let v = vec![19u8; 1024 * 1024];
        assert_eq!(v[999], 19);
        Box::leak(v.into_boxed_slice());
        DROP.with(|d| *d.borrow_mut() = Some(TlsDrop(address)));
        a[0].store(1, Ordering::SeqCst);
        while a[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
    }).unwrap();
}
"#;

/// A sync point waits as long as the fixture's compile takes; a hang shows as a hung test, not a timing failure.
fn wait(mut condition: impl FnMut() -> bool) {
    while !condition() {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn fixture() -> (Box<Unit>, u64, std::path::PathBuf) {
    fixture_of(FIXTURE, "fixture")
}

fn fixture_of(source: &str, entry_name: &str) -> (Box<Unit>, u64, std::path::PathBuf) {
    fixture_in("fixture.rs", source, entry_name)
}

fn fixture_in(file_name: &str, source: &str, entry_name: &str) -> (Box<Unit>, u64, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("relax-domain-teardown-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join(file_name);
    std::fs::write(&path, source).unwrap();
    // the std sources of this checkout (as lazy_reload_tests.rs does: a test binary is not next to them)
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let sess = std::sync::Arc::new(crate::Session::default());
    let (prog, core, _) = crate::load_with_std(&sess, path.to_str().unwrap(), "teardown_fixture", false).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let u = Unit::new(prog, tcx, core);
    let def = crate::program::DefId(u.prog.defs.iter().position(|d| u.prog.syms.str(d.name) == entry_name).unwrap() as u32);
    let fid = u.fn_id(crate::jit::FnKey::Inst(def, Vec::new()));
    let entry = u.entry(fid);
    (u, entry, root)
}

fn launch(u: &mut Unit, entry: u64, id: Option<DomainId>, flags: &[AtomicU64; 4]) -> (DomainId, u64, std::thread::JoinHandle<()>) {
    launch_with(u, entry, id, flags.as_ptr() as u64)
}

fn launch_with(u: &mut Unit, entry: u64, id: Option<DomainId>, arg: u64) -> (DomainId, u64, std::thread::JoinHandle<()>) {
    let id = prepare(&AppSpec {
        package: "teardown-fixture".into(),
        id,
        ..Default::default()
    })
    .unwrap();
    let loading = Loading(id);
    let base = create(u);
    let heap = crate::zone_alloc::domain::DomainHeap::new().unwrap();
    {
        let mut ds = domains();
        let d = ds.iter_mut().find(|d| d.info.id == id).unwrap();
        d.info.base = base;
        d.heap = Some(heap);
    }
    let h = launch_main(
        u as *mut Unit as usize,
        id,
        base,
        entry,
        arg,
        None,
        "teardown-fixture",
        None,
    )
    .unwrap();
    drop(loading);
    (id, base, h)
}

#[cfg(target_os = "linux")]
fn heap_in_use() -> u64 {
    let s = crate::inspect::mem::take(false, &mut Vec::new());
    s.nodes
        .iter()
        .find(|n| n.0 == "malloc/in use")
        .expect("Linux allocator accounting")
        .1
}

/// The heap check counts the whole process, so what grows it besides the fixture is settled first.
/// The lazy compiles of the fixture's code are one-off process state that the cycles would
/// otherwise make in whichever cycle first needs them, which depends on how the threads
/// interleave (and so on machine load). The tier workers also belong to the process:
/// their lazy start on a hot call must precede the heap baseline. (Nothing in the cycles reads
/// symbol tables: a stop reports no thread that is on its way out, so no stack is dumped.)
#[cfg(target_os = "linux")]
fn warm_process_state(u: &mut Unit, entry: u64) {
    crate::jit::precompile_reachable(u, entry);
}

#[test]
fn teardown_fixture_cycles_and_retiring_thread() {
    isolated_test!(teardown_fixture_cycles_and_retiring_thread);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture();
    let mut id = None;
    #[cfg(target_os = "linux")]
    let mut in_use = Vec::with_capacity(20);
    #[cfg(target_os = "linux")]
    warm_process_state(&mut u, entry);
    for _ in 0..20 {
        let flags = [const { AtomicU64::new(0) }; 4];
        let (domain, base, main) = launch(&mut u, entry, id, &flags);
        id = Some(domain);
        main.join().unwrap();
        wait(|| flags[0].load(Ordering::SeqCst) != 0);
        let live = info(domain).unwrap();
        assert_eq!(live.threads, 1, "main ended; child still owns the domain");
        assert!(live.thread_names.iter().any(|n| n == "fixture-worker"));
        assert!(live.arena_in_use >= 2 * 1024 * 1024);
        assert!(live.arena_committed > 0 && live.static_committed > 0);
        let before = freed_by_teardown();
        let stop = std::thread::spawn(move || stop(domain, Stop::Quit).unwrap());
        wait(|| cancelled(domain));
        assert!(blocks().contains(&base));
        flags[1].store(1, Ordering::SeqCst);
        stop.join().unwrap();
        // the worker outlives the app's main: its end (TLS drop, then teardown) completes the domain
        wait_thread_done(domain);
        assert_eq!(flags[2].load(Ordering::SeqCst), 1, "TLS drop ran before teardown");
        let dead = info(domain).unwrap();
        assert_eq!(
            (
                dead.base,
                dead.threads,
                dead.arena_in_use,
                dead.arena_committed,
                dead.static_committed
            ),
            (0, 0, 0, 0, 0)
        );
        assert!(!blocks().contains(&base));
        assert!(freed_by_teardown() > before);
        assert!(matches!(dead.state, DomainState::Exited(0)));
        #[cfg(target_os = "linux")]
        in_use.push(heap_in_use());
    }
    // Cycle 0 launches a new domain; from cycle 1 on the domain is cycled. What a cycle keeps is in
    // every sample after it; what the process's own work holds for a moment (under the JIT, a
    // compile's or a cache write's 256 KiB vector) is in some. So the lowest of the last cycles
    // against the lowest of the first: two MB leaked per cycle may not pass.
    #[cfg(target_os = "linux")]
    {
        let lowest = |s: &[u64]| s.iter().copied().min().unwrap();
        let (first, last) = (lowest(&in_use[1..6]), lowest(&in_use[15..]));
        assert!(last <= first + 256 * 1024, "cycled domains kept heap: {last} > {first}: {in_use:?}");
    }

    // No cooperative completion yet: retire without blocking the replacement,
    // retain every allocation until the registered thread and its TLS end.
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, base, main) = launch(&mut u, entry, id, &flags);
    main.join().unwrap();
    wait(|| flags[0].load(Ordering::SeqCst) != 0);
    let before = info(domain).unwrap();
    let freed = freed_by_teardown();
    retire(domain).unwrap();
    let retiring = info(domain).unwrap();
    assert_eq!(retiring.arena_in_use, before.arena_in_use);
    assert_eq!(retiring.arena_committed, before.arena_committed);
    assert_eq!(retiring.base, base);
    assert_eq!(freed_by_teardown(), freed);
    assert!(prepare(&AppSpec {
        id: Some(domain),
        ..Default::default()
    })
    .is_err());
    assert!(still_owned(domain).is_some_and(|why| why.contains("1 threads still alive: fixture-worker")));
    let next_flags = [const { AtomicU64::new(0) }; 4];
    let (replacement, next_base, next_main) = launch(&mut u, entry, None, &next_flags);
    assert_ne!(replacement, domain);
    assert_ne!(next_base, base);
    next_main.join().unwrap();
    wait(|| next_flags[0].load(Ordering::SeqCst) != 0);
    assert!(blocks().contains(&base));
    // The old thread still reads its own statics and runs its TLS destructor;
    // only its final registration releases the old mapping.
    flags[1].store(1, Ordering::SeqCst);
    wait_thread_done(domain);
    assert_eq!(flags[2].load(Ordering::SeqCst), 1);
    assert_eq!(still_owned(domain), None);
    assert!(!blocks().contains(&base));
    assert!(blocks().contains(&next_base));
    assert!(freed_by_teardown() > freed);
    next_flags[1].store(1, Ordering::SeqCst);
    stop(replacement, Stop::Quit).unwrap();
    // its main returned long ago, so the stop does not wait for the worker: the worker ends (its
    // TLS destructor on the replacement's heap) before the record and that heap go
    wait_thread_done(replacement);
    domains().retain(|d| d.info.id != domain && d.info.id != replacement);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_live_domain_only_receives_its_units_statics() {
    isolated_test!(a_live_domain_only_receives_its_units_statics);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(r#"
        use std::sync::atomic::{AtomicU64, Ordering};
        static FIRST: AtomicU64 = AtomicU64::new(0);
        static SECOND: AtomicU64 = AtomicU64::new(0);
        fn app_entry(p: *mut u64) {
            let flags = unsafe { &*(p as *const [AtomicU64; 4]) };
            FIRST.store(17, Ordering::SeqCst);
            SECOND.store(31, Ordering::SeqCst);
            flags[3].store(&FIRST as *const AtomicU64 as u64, Ordering::SeqCst);
            flags[0].store(1, Ordering::SeqCst);
            while flags[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
            let intact = FIRST.load(Ordering::SeqCst) == 17 && SECOND.load(Ordering::SeqCst) == 31;
            flags[2].store(if intact { 1 } else { 2 }, Ordering::SeqCst);
        }
    "#, "app_entry");
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, base, main) = launch(&mut u, entry, None, &flags);
    let mut auxiliary = None;
    // Even a failed image check must release and join the app before its flags or unit go away.
    let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wait(|| flags[0].load(Ordering::SeqCst) != 0 || main.is_finished());
        assert_eq!(flags[0].load(Ordering::SeqCst), 1, "app exited before publishing its static");
        assert!((base..base + BLOCK_SIZE as u64).contains(&flags[3].load(Ordering::SeqCst)), "app static outside its live block");

        const MARKER: u64 = 0x2222_2222_2222_2222;
        let (unit, other_entry, _) = fixture_in("other.rs", r#"
            static MARKER: [u64; 2] = [0x2222_2222_2222_2222; 2];
            fn marker_address(p: *mut u64) { unsafe { *p = &MARKER as *const [u64; 2] as u64; } }
        "#, "marker_address");
        let other = auxiliary.insert(unit).as_mut();
        other.aux = true;
        other.tier1 = None;
        let dom0 = other.syms.dom0();
        let mut addr = 0u64;
        unsafe { crate::jit::call_jit(other, other_entry, &mut addr as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
        assert!((dom0..=dom0 + BLOCK_SIZE as u64 - 8).contains(&addr), "auxiliary static outside its compile domain");
        assert_eq!(unsafe { (addr as *const u64).read_unaligned() }, MARKER, "auxiliary static's initial image");

        // Pin the mapping for these reads even if the app faults during its wait.
        let blocks = BLOCKS.lock().unwrap_or_else(|e| e.into_inner());
        assert!(blocks.iter().any(|&(b, _)| b == base), "app block reclaimed during the auxiliary call");
        let at = base + (addr - dom0);
        assert_ne!(unsafe { (at as *const u64).read_unaligned() }, MARKER, "auxiliary static copied to its offset in the app");
        let prefix = unsafe { std::slice::from_raw_parts(base as *const u8, 32 * 1024) };
        assert!(!prefix.windows(8).any(|word| word == MARKER.to_ne_bytes()), "auxiliary static found in the app's first 32 KiB");
        drop(blocks);

        flags[1].store(1, Ordering::SeqCst);
        wait(|| flags[2].load(Ordering::SeqCst) != 0 || main.is_finished());
        assert_eq!(flags[2].load(Ordering::SeqCst), 1, "the app's statics changed while it waited");
    }));
    if checked.is_err() { flags[1].store(1, Ordering::SeqCst); }
    main.join().unwrap();
    wait_thread_done(domain);
    assert!(!blocks().contains(&base));
    domains().retain(|d| d.info.id != domain);
    if let Some(mut other) = auxiliary {
        unsafe { crate::jit::retire_unit(&mut *other) };
    }
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
    if let Err(p) = checked { std::panic::resume_unwind(p); }
}

// The CPU budget's source and an app whose task pool lives in a static (tests/static_pool/pool.rs).
// The budget's `#[relax_api]` marks and its `relax_api::option_layouts!` entries name a crate a
// one-file program does not have; the entries are for its provided build, the fixture runs it as JIT code.
// So does its `extern crate makepad_thread_local`, whose `thread_local!` is std's in code Relax
// compiles: without it the budget's thread-locals are std's macro, the same declarations.
fn static_pool_fixture() -> String {
    let mut budget = include_str!("../../../../libs/cpu_budget/src/lib.rs")
        .replace("use relax_api::relax_api;\n", "")
        .replace("#[relax_api]\n", "")
        .replace("#[macro_use(thread_local)]\nextern crate makepad_thread_local;\n", "");
    while let Some(at) = budget.find("relax_api::option_layouts! {") {
        let end = at + budget[at..].find("\n}\n").expect("an option_layouts! block ends with a `}` line") + 3;
        budget.replace_range(at..end, "");
    }
    let app = include_str!("../tests/static_pool/pool.rs");
    format!("mod makepad_cpu_budget {{\n{budget}\n}}\n{app}")
}

#[test]
fn static_task_pool_relaunches_in_place_without_live_threads() {
    isolated_test!(static_task_pool_relaunches_in_place_without_live_threads);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(&static_pool_fixture(), "static_pool_app");
    let mut id = None;
    for _ in 0..3 {
        let flags = [const { AtomicU64::new(0) }; 4];
        let (domain, base, main) = launch(&mut u, entry, id, &flags);
        if let Some(previous) = id { assert_eq!(domain, previous); }
        id = Some(domain);
        wait(|| flags[0].load(Ordering::SeqCst) != 0);
        let live = info(domain).unwrap();
        for index in 0..3 {
            assert!(live.thread_names.contains(&format!("pool-{index}")));
        }
        flags[1].store(1, Ordering::SeqCst);
        main.join().unwrap();
        stop(domain, Stop::Quit).unwrap();
        // stop returns once the app's main has; the closed pool's workers end on their own
        wait_thread_done(domain);
        assert_eq!(flags[2].load(Ordering::SeqCst), 1);
        let dead = info(domain).unwrap();
        assert_eq!((dead.threads, dead.base, dead.arena_committed), (0, 0, 0));
        assert_eq!(still_owned(domain), None);
        assert!(!blocks().contains(&base));
    }
    domains().retain(|d| Some(d.info.id) != id);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_during_compile_never_enters_main() {
    isolated_test!(cancel_during_compile_never_enters_main);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    for phase in ["resolve", "build scripts", "load", "collect", "link", "main"] {
        let id = prepare(&AppSpec {
            package: "cancel-fixture".into(),
            ..Default::default()
        })
        .unwrap();
        let loading = Loading(id);
        compile_phase(id, phase).unwrap();
        retire(id).unwrap();
        assert!(compile_phase(id, "next").is_err());
        // Exercise the production loader entry: cancellation must return before it even
        // dereferences the unit, and therefore before resolve, linking or main can run.
        assert_eq!(start_prepared(std::ptr::null_mut(), AppSpec::default(), id).unwrap(), id);
        drop(loading);
        let dead = info(id).unwrap();
        assert_eq!(dead.state, DomainState::Stopped);
        assert_eq!(dead.base, 0);
        assert_eq!(dead.threads, 0);
        assert_eq!(dead.arena_committed, 0);
        domains().retain(|d| d.info.id != id);
    }
}

struct LoadFixture {
    dir: std::path::PathBuf,
    unit: Box<Unit>,
    cfg: crate::cfg::CfgSet,
}

impl LoadFixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("relax-domain-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::dirs::set_relax_dir(dir.join("run"));
        let mut session = crate::Session::default();
        session.options.ctfe = crate::options::Ctfe::Off;
        let session = std::sync::Arc::new(session);
        let cfg = crate::cfg::CfgSet::host(&session, &[]);
        let mut prog = crate::program::Program::new(&session);
        // Domain graphs start after the boot's std crates. Empty one-file versions suffice
        // for these no_std libraries, whose only function takes and returns a primitive.
        let std = crate::std_pre_crates(&session, prog.std_simd, prog.std_overflow_checks);
        for name in std.into_iter().map(|c| c.name).chain(std::iter::once("host".into())) {
            let path = dir.join(format!("{name}.rs"));
            std::fs::write(&path, "#![no_std]\n").unwrap();
            prog.load_crate(&name, path.to_str().unwrap(), 2021, cfg.clone(), vec![]).unwrap();
        }
        prog.resolve_imports();
        assert!(prog.errors.is_empty(), "{:?}", prog.errors);
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
        Self { dir, unit: Unit::new(prog, tcx, 0), cfg }
    }

    fn chain(&self, names: &[&str]) -> TestLoad {
        self.chain_with(names, "")
    }

    /// `chain`, with `first` added to the first crate's source.
    fn chain_with(&self, names: &[&str], first: &str) -> TestLoad {
        let mut crates = Vec::new();
        let mut pkg = Vec::new();
        for (i, &name) in names.iter().enumerate() {
            let path = self.dir.join(format!("{name}.rs"));
            let mut deps = Vec::new();
            let value = if i == 0 {
                "x".to_owned()
            } else {
                let dep = names[i - 1];
                deps.push((i - 1, dep.to_owned()));
                format!("{dep}::value(x)")
            };
            let extra = if i == 0 { first } else { "" };
            std::fs::write(&path, format!("#![no_std]\n{extra}pub fn value(x: u32) -> u32 {{ {value} }}\n")).unwrap();
            crates.push(crate::crates::CrateUnit {
                name: name.into(), src_path: path.to_str().unwrap().into(), edition: 2021,
                deps, features: vec![], host: None, proc_macro: false, bin: false, harness: false,
                build_script: false, version: "0.1.0".into(), registry: false, host_only: false,
            });
            pkg.push((self.dir.to_str().unwrap().into(), name.into(), "0.1.0".into()));
        }
        let n = names.len();
        let graph = crate::unitgraph::Graph {
            crates, root: n - 1, build_scripts: vec![], pkg, cfgs: vec![vec![]; n],
            env: vec![vec![]; n], link_libs: vec![], link_search: Vec::new(), workspace_root: String::new(), native: None, raw: vec![], bins: vec![],
        };
        let order = crate::crates::topo_order(&graph.crates);
        let cfgs = vec![self.cfg.clone(); n];
        let idents = identities(&graph, &cfgs, &order);
        let id = prepare(&AppSpec { package: names[n - 1].into(), ..Default::default() }).unwrap();
        TestLoad { graph, order, cfgs, idents, id }
    }

    fn address(&mut self) -> usize { &mut *self.unit as *mut Unit as usize }
}

impl Drop for LoadFixture {
    fn drop(&mut self) {
        LOADED.lock().unwrap().retain(|r| !std::path::Path::new(&r.4).starts_with(&self.dir));
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

struct TestLoad {
    graph: crate::unitgraph::Graph,
    order: Vec<usize>,
    cfgs: Vec<crate::cfg::CfgSet>,
    idents: Vec<u64>,
    id: DomainId,
}

impl TestLoad {
    fn run(&self, up: usize) -> Result<(u32, usize, Vec<String>), Vec<String>> {
        unsafe { load_graph(up as *mut Unit, &self.graph, &self.order, &self.cfgs, &self.idents, self.id, true) }
    }
}

impl Drop for TestLoad {
    fn drop(&mut self) {
        drop(Loading(self.id));
        domains().retain(|d| d.info.id != self.id);
    }
}

// Drop inside the thread scope, so a failed assertion also releases the loaders before join.
struct HoldSteps;
impl HoldSteps {
    fn new() -> Self { hold_load_steps(true); Self }
}
impl Drop for HoldSteps {
    fn drop(&mut self) { hold_load_steps(false); }
}

fn hold_after_first_crate() {
    assert_eq!(wait_load_step_held(), 0);
    release_load_step();
    assert_eq!(wait_load_step_held(), 1);
}

fn value_def(u: &Unit, krate: u32) -> Option<crate::program::DefId> {
    u.prog.defs.iter().enumerate().find_map(|(i, d)| {
        (d.krate == krate && u.prog.syms.str(d.name) == "value").then_some(crate::program::DefId(i as u32))
    })
}

fn shared_dependency(u: &Unit, root: u32) -> u32 {
    u.prog.crates[root as usize].deps.iter().find(|(name, _)| u.prog.syms.str(*name) == "a").unwrap().1
}

fn records_for(ident: u64) -> usize {
    LOADED.lock().unwrap().iter().filter(|r| r.0 == ident).count()
}

#[test]
fn a_waiting_request_is_served_between_load_steps() {
    isolated_test!(a_waiting_request_is_served_between_load_steps);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let graph = f.chain(&["a", "b", "c"]);
    let up = f.address();
    std::thread::scope(|s| {
        let _hold = HoldSteps::new();
        let loader = s.spawn(|| graph.run(up));
        for k in 0..graph.order.len() {
            assert_eq!(wait_load_step_held(), k);
            let (tx, rx) = std::sync::mpsc::channel();
            // This thread's request is queued before the loader is free to post its next one.
            crate::owner::post(crate::owner::Class::Program, move || tx.send(STEPS_BEGUN.load(Ordering::SeqCst)).unwrap());
            release_load_step();
            assert_eq!(rx.recv().unwrap(), k, "a waiting request was overtaken at step {k}");
        }
        let (_, reused, new) = loader.join().unwrap().unwrap();
        assert_eq!((reused, new.len()), (0, 3));
        assert_eq!(STEPS_BEGUN.load(Ordering::SeqCst), 3);
    });
}

#[test]
fn a_load_cancelled_between_steps_keeps_only_whole_crates() {
    isolated_test!(a_load_cancelled_between_steps_keeps_only_whole_crates);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let graph = f.chain(&["a", "b", "c"]);
    let up = f.address();
    let err = std::thread::scope(|s| {
        let hold = HoldSteps::new();
        let loader = s.spawn(|| graph.run(up));
        hold_after_first_crate();
        retire(graph.id).unwrap();
        drop(hold);
        loader.join().unwrap().unwrap_err()
    });
    assert_eq!(err, ["domain compilation cancelled"]);
    let a = lookup_loaded(graph.idents[0]).expect("the completed crate is recorded");
    assert!(value_def(&f.unit, a).is_some(), "the completed crate has its definitions");
    assert_eq!(lookup_loaded(graph.idents[1]), None, "b started after cancellation");
    assert_eq!(lookup_loaded(graph.idents[2]), None, "c started after cancellation");
}

#[test]
fn a_parse_error_keeps_the_crates_before_it() {
    isolated_test!(a_parse_error_keeps_the_crates_before_it);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let bad = f.chain(&["a", "b", "c"]);
    std::fs::write(&bad.graph.crates[1].src_path, "#![no_std]\npub fn broken(\n").unwrap();
    let up = f.address();
    let err = std::thread::scope(|s| s.spawn(|| bad.run(up)).join().unwrap().unwrap_err());
    assert!(err.iter().any(|e| e.contains("b.rs")), "expected a parse error in b: {err:?}");
    let a = lookup_loaded(bad.idents[0]).expect("a was recorded before b failed");
    assert_eq!(lookup_loaded(bad.idents[1]), None, "the failing crate was recorded");
    assert_eq!(lookup_loaded(bad.idents[2]), None, "the load continued past the failing crate");

    let good = f.chain(&["a", "e"]);
    let (root, reused, _) = std::thread::scope(|s| s.spawn(|| good.run(up)).join().unwrap().unwrap());
    assert_eq!(reused, 1);
    assert_eq!(shared_dependency(&f.unit, root), a, "e must depend on the first load's a");
    assert_eq!(lookup_loaded(good.idents[0]), Some(a));
    assert_eq!(records_for(good.idents[0]), 1);
}

#[test]
fn a_recorded_crate_with_extern_crate_aliases_is_reused() {
    isolated_test!(a_recorded_crate_with_extern_crate_aliases_is_reused);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    // the alias adds an entry to a's dependency list beside the std crates
    let alias = "extern crate core as __core;\n";
    let ab = f.chain_with(&["a", "b"], alias);
    let up = f.address();
    std::thread::scope(|s| s.spawn(|| ab.run(up)).join().unwrap().unwrap());
    let a = lookup_loaded(ab.idents[0]).expect("a was recorded");
    let ad = f.chain_with(&["a", "d"], alias);
    let (root, reused, new) = std::thread::scope(|s| s.spawn(|| ad.run(up)).join().unwrap().unwrap());
    assert_eq!((reused, new.len()), (1, 1), "a was loaded again: {new:?}");
    assert_eq!(shared_dependency(&f.unit, root), a);
    assert_eq!(records_for(ab.idents[0]), 1);
}

#[test]
fn a_crate_recorded_by_a_cancelled_load_links_for_the_load_that_reuses_it() {
    isolated_test!(a_crate_recorded_by_a_cancelled_load_links_for_the_load_that_reuses_it);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let link = "#[link(name = \"step_a\")] extern \"C\" {}\n";
    let abc = f.chain_with(&["a", "b", "c"], link);
    let up = f.address();
    let err = std::thread::scope(|s| {
        let hold = HoldSteps::new();
        let loader = s.spawn(|| abc.run(up));
        hold_after_first_crate();
        retire(abc.id).unwrap();
        drop(hold);
        loader.join().unwrap().unwrap_err()
    });
    assert_eq!(err, ["domain compilation cancelled"]);
    let a = lookup_loaded(abc.idents[0]).expect("a was recorded before the cancel");
    // the cancelled load never linked: the next domain's link must find a's library
    let ad = f.chain_with(&["a", "d"], link);
    let (root, reused, _) = std::thread::scope(|s| s.spawn(|| ad.run(up)).join().unwrap().unwrap());
    assert_eq!(reused, 1);
    assert_eq!(shared_dependency(&f.unit, root), a, "d reuses the cancelled load's a");
    let libs = crate::run::link_attr_libs(&f.unit.prog, Some(&domain_crates(&f.unit.prog, root)));
    assert_eq!(libs, [("step_a".to_owned(), false)], "the reused crate's library is linked");
}

#[test]
fn two_loads_of_a_shared_crate_load_it_once() {
    isolated_test!(two_loads_of_a_shared_crate_load_it_once);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let ab = f.chain(&["a", "b"]);
    let ad = f.chain(&["a", "d"]);
    for load in [&ab, &ad] {
        use std::io::Write;
        let root = &load.graph.crates[load.graph.root];
        let mut file = std::fs::OpenOptions::new().append(true).open(&root.src_path).unwrap();
        writeln!(file, "#[link(name = \"step_{}\")] extern \"C\" {{}}", root.name).unwrap();
    }
    let up = f.address();
    let (b, d) = std::thread::scope(|s| {
        let hold = HoldSteps::new();
        let first = s.spawn(|| ab.run(up));
        assert_eq!(wait_load_step_held(), 0);
        let second = s.spawn(|| ad.run(up));
        // Both have examined the empty registry. Run the two steps for a before either root.
        assert_eq!(wait_for_loaders(2), 0);
        release_load_step();
        assert_eq!(wait_for_loaders(2), 0);
        release_load_step();
        assert_eq!(wait_for_loaders(2), 1);
        drop(hold);
        (first.join().unwrap().unwrap(), second.join().unwrap().unwrap())
    });
    let a = lookup_loaded(ab.idents[0]).expect("the shared crate was recorded");
    assert_eq!((b.1, d.1), (0, 1), "the second load reused a");
    assert_eq!(records_for(ab.idents[0]), 1);
    assert_eq!(shared_dependency(&f.unit, b.0), a);
    assert_eq!(shared_dependency(&f.unit, d.0), a);
    for (root, lib) in [(b.0, "step_b"), (d.0, "step_d")] {
        let selected = HashSet::from([a, root]);
        let libs = crate::run::link_attr_libs(&f.unit.prog, Some(&selected));
        assert_eq!(libs, [(lib.to_owned(), false)], "a domain links only its own root's library");
    }
}

#[test]
fn a_crate_loaded_by_an_earlier_step_compiles_before_the_load_ends() {
    isolated_test!(a_crate_loaded_by_an_earlier_step_compiles_before_the_load_ends);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let mut f = LoadFixture::new();
    let graph = f.chain(&["a", "b", "c"]);
    let up = f.address();
    std::thread::scope(|s| {
        let hold = HoldSteps::new();
        let loader = s.spawn(|| graph.run(up));
        hold_after_first_crate();
        let a = lookup_loaded(graph.idents[0]).expect("a is already recorded");
        let installed = crate::owner::call(crate::owner::Class::Program, || unsafe {
            let up = up as *mut Unit;
            let def = value_def(&*up, a).expect("a's definition is visible");
            let slot = (*up).fn_id(crate::jit::FnKey::Inst(def, vec![]));
            let (code, map, relocs) = crate::jit::compile_fn(up, slot)?;
            (*up).install(slot, &code, &relocs, map, 0)
        }).unwrap();
        assert_ne!(installed, 0);
        assert_eq!(STEPS_BEGUN.load(Ordering::SeqCst), 1, "the install finished before b began");
        drop(hold);
        let (_, _, new) = loader.join().unwrap().unwrap();
        assert_eq!(new.len(), 3);
    });
}

/// A domain loader holds its compile job for the whole load, build scripts included: it waits
/// for its build-script threads, and each starts the script's process. Where std forks for that
/// (Android; here: pre_exec forces the fork path), a fork hook that waited for every compile job
/// waited for the loader's own job, which waited for the fork (the Pixel WM hung at
/// 'compiling: build scripts' for every app not yet running). The loader's own process starts
/// never pause compile jobs; only the program's fork (the loader's `fork` symbol) does.
#[cfg(unix)]
#[test]
fn a_build_script_start_under_a_loading_domain_finishes() {
    isolated_test!(a_build_script_start_under_a_loading_domain_finishes);
    use std::os::unix::process::CommandExt;
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // a real unit: the process is set up as `makepad run` sets it up
    let (u, _entry, root) = fixture();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _loading = crate::jit::compile_job();
        let status = std::thread::scope(|s| {
            s.spawn(|| {
                let mut script = std::process::Command::new("true");
                unsafe { script.pre_exec(|| Ok(())) };
                script.status()
            }).join()
        });
        let _ = done_tx.send(status.map(|s| s.map(|s| s.success())));
    });
    // (a process start that waits for the loader's compile job, the deadlock this is about,
    // holds the test here, a hung test)
    match done_rx.recv() {
        Ok(Ok(Ok(true))) => {}
        Ok(r) => panic!("the build script did not run: {:?}", r.map(|r| r.map_err(|e| e.to_string()))),
        Err(_) => panic!("the process start's thread ended without an answer"),
    }
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// A model-loading worker of an app domain (the on-device Translator's 'llm-turns'): a thread JIT
// code starts in the domain, allocating large buffers (several 2 MB chunks and multi-chunk runs,
// grown by reallocation) next to thousands of small named records, and reading fields of those
// records through returned references (`match src(i)?.desc.ty`). Relax took such a field for a
// value of the statement and dropped a copy of the record's `desc` after the match: the name
// String was freed while the record still owned it, and the domain arena's free list later
// handed out a live block (the phone WM died in zone_alloc on 'llm-turns', fault address the
// ASCII of a tensor name).
const WORKER_FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
#[derive(Clone, Copy, PartialEq)]
enum Ty { F32, F16, I32 }
struct Desc { name: Option<String>, ty: Ty }
struct Tensor { desc: Desc, src: [Option<usize>; 2], bytes: usize }
fn select(ts: &[Tensor], t: &Tensor) -> Result<u64, String> {
    let src = |i: usize| -> Result<&Tensor, String> {
        t.src[i].and_then(|id| ts.get(id)).ok_or_else(|| format!("{:?}: missing src{i}", t.desc.name))
    };
    Ok(match src(0)?.desc.ty {
        Ty::F32 => 1,
        ty => match src(1)?.desc.ty { Ty::I32 => 2, _ if ty == Ty::F16 => 3, _ => 4 },
    })
}
fn load(rounds: usize) -> u64 {
    let mut acc = 0u64;
    for round in 0..rounds {
        // weights-like buffers: one grown by reallocation past several chunks, two allocated whole
        let mut grown: Vec<u8> = Vec::new();
        for i in 0..(5usize << 20) { grown.push((i ^ round) as u8); }
        let whole = vec![round as u8; 9 << 20];
        let zeroed = vec![0u64; 3 << 20];
        let mut ts: Vec<Tensor> = Vec::new();
        for i in 0..3000usize {
            let ty = [Ty::F32, Ty::F16, Ty::I32][i % 3];
            let name = format!("hybrid_decode.layer{}.attn_kq_mask_{}", i / 40, i % 40);
            let src = [if i > 0 { Some(i - 1) } else { None }, if i > 1 { Some(i / 2) } else { None }];
            ts.push(Tensor { desc: Desc { name: Some(name), ty }, src, bytes: i * 64 });
        }
        for t in ts.iter().skip(2) {
            acc += select(&ts, t).unwrap();
        }
        for t in &ts {
            let n = t.desc.name.as_deref().unwrap();
            assert!(n.starts_with("hybrid_decode.layer"), "a name was freed while its record owned it: {n:?}");
            acc += t.bytes as u64 % 7;
        }
        acc += grown[(3 << 20) + 1] as u64 + whole[8 << 20] as u64 + zeroed[(3 << 20) - 1];
    }
    acc
}
fn fixture(p: *mut u64) {
    let address = p as usize;
    std::thread::Builder::new().name("llm-turns".into()).spawn(move || {
        let a = unsafe { &*(address as *const [AtomicU64; 4]) };
        a[0].store(load(3), Ordering::SeqCst);
        a[1].store(1, Ordering::SeqCst);
    }).unwrap();
}
"#;

#[test]
fn a_domain_worker_with_large_allocations_and_borrowed_field_matches() {
    isolated_test!(a_domain_worker_with_large_allocations_and_borrowed_field_matches);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(WORKER_FIXTURE, "fixture");
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    main.join().unwrap();
    wait(|| flags[1].load(Ordering::SeqCst) != 0 || info(domain).unwrap().threads == 0);
    assert_eq!(flags[1].load(Ordering::SeqCst), 1, "the worker did not finish: {:?}", info(domain).unwrap().state);
    // per round: select over 2998 records ((i-1) % 3 picks F32 -> 1, F16 -> 3 or 2, I32 -> 4 or 2),
    // bytes % 7 over 3000 records, and the three buffer reads
    let mut expected = 0u64;
    for round in 0..3u64 {
        let ty = |i: usize| i % 3;
        for i in 2..3000usize {
            let (s0, s1) = (ty(i - 1), ty(i / 2));
            expected += match s0 { 0 => 1, _ if s1 == 2 => 2, 1 => 3, _ => 4 };
        }
        expected += (0..3000u64).map(|i| i * 64 % 7).sum::<u64>();
        expected += (((3u64 << 20) + 1) ^ round) & 0xff;
        expected += round;
    }
    assert_eq!(flags[0].load(Ordering::SeqCst), expected);
    wait(|| info(domain).unwrap().threads == 0);
    stop(domain, Stop::Quit).unwrap();
    let dead = info(domain).unwrap();
    assert_eq!((dead.threads, dead.arena_in_use, dead.arena_committed), (0, 0, 0));
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// Windows: the JIT's thread-local key lands past the TEB's 64 direct TlsSlots when the process
// already used them up (a GPU driver loaded before Relax creates its key: box3d on a visible
// window). The test uses up the direct slots before anything creates the key, then runs JIT code
// that reads its thread-local block on the calling thread and on a new one (whose
// TlsExpansionSlots array is still null: the slow path). Elsewhere it runs the same fixture.
// Everywhere: a thread-local's destructor at the new thread's end reads the environment as the
// thread did before (the host's state for that read outlives every destructor: pmhost).
const TLS_FIXTURE: &str = r#"
use std::cell::Cell;
thread_local! { static N: Cell<u64> = const { Cell::new(0) }; }
fn bump(k: u64) -> u64 {
    N.with(|n| { n.set(n.get() + k); n.get() })
}
fn path_hash() -> u64 {
    std::env::var("PATH").map_or(0, |v| v.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)) | 1)
}
struct ReadsAtEnd(usize);
impl Drop for ReadsAtEnd {
    fn drop(&mut self) { unsafe { *(self.0 as *mut u64).add(3) = path_hash(); } }
}
thread_local! { static AT_END: std::cell::RefCell<Option<ReadsAtEnd>> = const { std::cell::RefCell::new(None) }; }
fn tls_fixture(p: *mut u64) {
    let a = bump(3) + bump(4);
    let at = p as usize;
    let b = std::thread::spawn(move || {
        // registered before the thread's first environment read: its drop comes after the
        // host's own thread-locals of that read are destroyed
        AT_END.with(|e| *e.borrow_mut() = Some(ReadsAtEnd(at)));
        unsafe { *(at as *mut u64).add(2) = path_hash(); }
        bump(5) + bump(6)
    }).join().unwrap();
    unsafe { *p = a * 100 + b; *p.add(1) = bump(0); }
}
"#;

#[test]
fn jit_tls_past_the_direct_slots() {
    isolated_test!(jit_tls_past_the_direct_slots);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(windows)]
    {
        let mut last = 0;
        while last < 64 {
            last = unsafe { crate::os::win32::TlsAlloc() };
            assert_ne!(last, u32::MAX, "TlsAlloc failed");
        }
    }
    let (mut u, entry, root) = fixture_of(TLS_FIXTURE, "tls_fixture");
    // (written by the JIT through the pointer: a `mut` array, or the reads below fold to zeros)
    let mut out = [0u64; 4];
    unsafe { crate::jit::call_jit(&mut *u, entry, out.as_mut_ptr() as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
    assert_eq!(out[..2], [10 * 100 + 16, 7]);
    assert_ne!(out[2], 0, "the new thread read PATH");
    assert_eq!(out[3], out[2], "a thread-local's destructor at its thread's end reads the environment as the thread did");
    let _ = std::fs::remove_dir_all(root);
}

// A callback the domain's code hands to native code runs on a thread native code owns (the HAL's
// realtime audio thread): through foreign_enter it sees the domain's statics, not the default
// domain's copy, and allocates in the domain's heap; foreign_leave puts the thread back.
const FOREIGN_FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
static TAPPED: AtomicU64 = AtomicU64::new(0);
static mut LAST: Option<Box<[u8; 4096]>> = None;
extern "C" fn tap(n: u64) {
    unsafe { LAST = Some(Box::new([n as u8; 4096])); }
    TAPPED.fetch_add(n, Ordering::SeqCst);
}
fn foreign(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let f: extern "C" fn(u64) = tap;
    a[0].store(f as usize as u64, Ordering::SeqCst);
    while TAPPED.load(Ordering::SeqCst) < 6 { std::thread::yield_now(); }
    a[1].store(TAPPED.load(Ordering::SeqCst), Ordering::SeqCst);
    a[2].store(unsafe { LAST.as_ref().map_or(0, |b| b[0] as u64) }, Ordering::SeqCst);
}
"#;

#[test]
fn foreign_thread_callback_sees_domain_statics() {
    isolated_test!(foreign_thread_callback_sees_domain_statics);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(FOREIGN_FIXTURE, "foreign");
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, base, main) = launch(&mut u, entry, None, &flags);
    wait(|| flags[0].load(Ordering::SeqCst) != 0);
    let tap: extern "C" fn(u64) = unsafe { std::mem::transmute(flags[0].load(Ordering::SeqCst) as usize) };
    let scope = foreign_scope_of(base);
    assert_ne!(scope, 0);
    // a thread the program never started, as a device's callback thread
    std::thread::spawn(move || {
        for n in 1..=3 {
            let saved = foreign_enter(scope);
            assert_eq!(current(), base);
            tap(n);
            foreign_leave(saved);
            assert_ne!(current(), base, "leave puts the thread back");
            assert!(crate::zone_alloc::domain::current().is_null(), "and its heap");
        }
    })
    .join()
    .unwrap();
    // (a callback that wrote the default domain's copy leaves the program, and this wait, hanging)
    wait(|| flags[1].load(Ordering::SeqCst) != 0);
    main.join().unwrap();
    assert_eq!(flags[1].load(Ordering::SeqCst), 6, "the program saw every callback's write");
    assert_eq!(flags[2].load(Ordering::SeqCst), 3);
    wait(|| info(domain).is_some_and(|i| i.threads == 0 && i.base == 0));
    assert!(!blocks().contains(&base));
    // a stale scope enters nothing
    assert_eq!(foreign_enter(scope).into_words()[2], 0);
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// A callback native code calls through `app_call` alone, with no scope recorded where the app
// registered it (a script VM's waker, a UI router), on a thread the program never started.
const APP_CALL_FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
static TAPPED: AtomicU64 = AtomicU64::new(0);
static mut LAST: Option<Box<[u8; 4096]>> = None;
extern "C" fn tap(n: u64) -> u64 {
    let b = Box::new([n as u8; 4096]);
    let at = &*b as *const [u8; 4096] as usize as u64;
    unsafe { LAST = Some(b); }
    TAPPED.fetch_add(n, Ordering::SeqCst);
    at
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let f: extern "C" fn(u64) -> u64 = tap;
    a[0].store(f as usize as u64, Ordering::SeqCst);
    while a[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
    a[2].store(TAPPED.load(Ordering::SeqCst), Ordering::SeqCst);
    a[3].store(unsafe { LAST.as_ref().map_or(0, |b| b[0] as u64) }, Ordering::SeqCst);
}
"#;

/// A thread in no domain calls into the run's app: the call writes the app's static (the app's
/// thread reads every write), its Box is in the app's heap, and the thread is back in no domain
/// after it.
#[test]
fn a_native_thread_in_no_domain_calls_into_the_runs_app() {
    isolated_test!(a_native_thread_in_no_domain_calls_into_the_runs_app);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(APP_CALL_FIXTURE, "fixture");
    assert!(crate::zone_alloc::compiler_arena_enabled(), "premise: heaps are routed");
    // the run's app, named before it launches (as the host does)
    const APP: DomainId = 7;
    crate::run::set_app_domain(APP);
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, _base, main) = launch(&mut u, entry, Some(APP), &flags);
    wait(|| flags[0].load(Ordering::SeqCst) != 0);
    let (lo, len) = domains().iter().find(|d| d.info.id == domain).and_then(|d| d.heap.as_deref()).expect("the app's heap").range();
    let tap = flags[0].load(Ordering::SeqCst);
    // tap, n -> the Box's address
    extern "C" fn thunk(p: *mut u8) {
        let c = unsafe { &mut *(p as *mut [u64; 3]) };
        let tap: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(c[0] as usize) };
        c[2] = tap(c[1]);
    }
    let made = std::thread::spawn(move || {
        (1..=3u64).map(|n| {
            let mut c = [tap, n, 0];
            assert!(crate::jit::app_call(thunk, &mut c as *mut [u64; 3] as *mut u8));
            assert_eq!(current(), default_base(), "the call leaves the thread in no domain");
            assert!(crate::zone_alloc::domain::current().is_null(), "and with no heap");
            c[2]
        }).collect::<Vec<_>>()
    }).join().unwrap();
    flags[1].store(1, Ordering::SeqCst);
    main.join().unwrap();
    assert_eq!(flags[2].load(Ordering::SeqCst), 6, "the app's static holds every native call's write");
    assert_eq!(flags[3].load(Ordering::SeqCst), 3);
    let in_heap = |a: u64| (lo as u64..(lo + len) as u64).contains(&a);
    for boxed in made {
        assert!(in_heap(boxed), "Box {boxed:#x} outside the app's heap");
    }
    wait(|| info(domain).is_some_and(|i| i.threads == 0 && i.base == 0));
    domains().retain(|d| d.info.id != domain);
    crate::run::set_app_domain(0);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// The program's thread-local on a thread the program never started.
const STAYING_THREAD: &str = r#"
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
thread_local! { static CALLS: Cell<u64> = const { Cell::new(0) }; }
extern "C" fn tap(out: *mut u64) {
    CALLS.with(|c| {
        unsafe { *out = c.get() };
        c.set(c.get() + 1);
    });
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let f: extern "C" fn(*mut u64) = tap;
    a[0].store(f as usize as u64, Ordering::SeqCst);
    while a[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
}
"#;

/// A native thread stays through a run's end and calls into the next run: the next run's
/// thread-locals start empty on it, as on a new thread, whatever the last run left there.
#[test]
fn a_thread_that_stays_meets_the_next_run_as_a_new_thread() {
    isolated_test!(a_thread_that_stays_meets_the_next_run_as_a_new_thread);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    extern "C" fn thunk(p: *mut u8) {
        let (tap, out) = unsafe { *(p as *const (usize, *mut u64)) };
        let tap: extern "C" fn(*mut u64) = unsafe { std::mem::transmute(tap) };
        tap(out);
    }
    let (asks, asked) = std::sync::mpsc::channel::<(u64, usize)>();
    let (answers, answered) = std::sync::mpsc::channel::<u64>();
    let worker = std::thread::spawn(move || {
        for (scope, tap) in asked {
            let mut out = u64::MAX;
            let saved = foreign_enter(scope);
            let mut call = (tap, &mut out as *mut u64);
            assert!(crate::jit::app_call(thunk, &mut call as *mut _ as *mut u8));
            foreign_leave(saved);
            answers.send(out).unwrap();
        }
    });
    let mut units = Vec::new();
    for _run in 0..2 {
        let (mut u, entry, _root) = fixture_of(STAYING_THREAD, "fixture");
        u.tier1 = None;
        let up = &mut *u as *mut Unit;
        let flags: [AtomicU64; 4] = Default::default();
        let (domain, base, main) = launch(&mut u, entry, None, &flags);
        wait(|| flags[0].load(Ordering::SeqCst) != 0);
        let tap = flags[0].load(Ordering::SeqCst) as usize;
        let scope = foreign_scope_of(base);
        asks.send((scope, tap)).unwrap();
        assert_eq!(answered.recv().unwrap(), 0, "the run's first call on this thread");
        asks.send((scope, tap)).unwrap();
        assert_eq!(answered.recv().unwrap(), 1, "its second");
        flags[1].store(1, Ordering::SeqCst);
        main.join().unwrap();
        wait_thread_done(domain);
        let end = end_run(up, Duration::from_secs(10));
        assert!(end.left.is_empty(), "left: {:?}", end.left);
        crate::jit::thread_locals_end();
        unsafe { crate::os::tls::reset_statics() };
        units.push(u);
    }
    drop(asks);
    worker.join().unwrap();
    std::mem::forget(units);
}

// An app's `process::exit` from inside its event handler: the event loop runs each turn under
// catch_unwind (as the HAL's turn guard does), here two deep, with a RefCell borrowed across the
// exit. The exit is no panic: neither catch_unwind contains it, no later turn runs (each would
// panic on the borrow left behind), and the domain ends Exited with the program's code.
const EXIT_FIXTURE: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
use std::panic::{catch_unwind, AssertUnwindSafe};
fn exit_in_handler(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let view = std::cell::RefCell::new(0u64);
    for turn in 0..8u64 {
        let outer = catch_unwind(AssertUnwindSafe(|| {
            let inner = catch_unwind(AssertUnwindSafe(|| {
                let mut v = view.borrow_mut();
                *v += 1;
                a[0].fetch_add(1, Ordering::SeqCst);
                if turn == 2 { std::process::exit(7); }
            }));
            if inner.is_err() { a[1].fetch_add(1, Ordering::SeqCst); }
        }));
        if outer.is_err() { a[1].fetch_add(1, Ordering::SeqCst); }
    }
    a[2].store(1, Ordering::SeqCst);
}
"#;

#[test]
fn process_exit_in_a_caught_handler_ends_the_domain_with_its_code() {
    isolated_test!(process_exit_in_a_caught_handler_ends_the_domain_with_its_code);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(EXIT_FIXTURE, "exit_in_handler");
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    main.join().unwrap();
    let state = info(domain).unwrap().state;
    assert!(matches!(state, DomainState::Exited(7)), "the domain ended {state:?}");
    assert_eq!(flags[0].load(Ordering::SeqCst), 3, "turns ran after the exit");
    assert_eq!(flags[1].load(Ordering::SeqCst), 0, "a catch_unwind contained the exit");
    assert_eq!(flags[2].load(Ordering::SeqCst), 0, "the event loop went on past the exit");
    wait(|| info(domain).unwrap().threads == 0);
    stop(domain, Stop::Quit).unwrap();
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// ---- a run's end: every thread ends wherever it is, nothing of the program runs, nothing is left

const ENDED_RUN: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
extern "C" { fn pipe(fds: *mut i32) -> i32; fn read(fd: i32, buf: *mut u8, n: usize) -> isize; }
// flags[0]: threads at their place; flags[1]: a destructor of the program ran
fn flags(a: usize) -> &'static [AtomicU64; 4] { unsafe { &*(a as *const [AtomicU64; 4]) } }
struct MustNotRun(usize);
impl Drop for MustNotRun {
    fn drop(&mut self) {
        flags(self.0)[1].fetch_add(1, Ordering::SeqCst);
    }
}
thread_local! { static TLS: std::cell::RefCell<Option<MustNotRun>> = std::cell::RefCell::new(None); }
fn at_place(a: usize) {
    TLS.with(|t| *t.borrow_mut() = Some(MustNotRun(a)));
    flags(a)[0].fetch_add(1, Ordering::SeqCst);
}
fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new().name(name.into()).spawn(f).unwrap();
}
fn fixture(p: *mut u64) {
    let a = p as usize;
    spawn("in-jit", move || { at_place(a); let mut x = 1u64; loop { x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7)); } });
    spawn("parked", move || { at_place(a); loop { std::thread::park(); } });
    spawn("sleeping", move || { at_place(a); loop { std::thread::sleep(std::time::Duration::from_secs(3600)); } });
    spawn("condvar", move || {
        at_place(a);
        let pair = (std::sync::Mutex::new(()), std::sync::Condvar::new());
        let mut g = pair.0.lock().unwrap();
        loop { g = pair.1.wait(g).unwrap(); }
    });
    spawn("holds-lock", move || {
        at_place(a);
        let m = Box::leak(Box::new(std::sync::Mutex::new(0u64)));
        let _g = m.lock().unwrap();
        loop { std::thread::park(); }
    });
    #[cfg(unix)]
    spawn("in-read", move || {
        at_place(a);
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        let mut b = [0u8; 8];
        loop { unsafe { read(fds[0], b.as_mut_ptr(), 8) }; }
    });
    spawn("in-accept", move || {
        at_place(a);
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        loop { let _ = l.accept(); }
    });
    spawn("caught", move || {
        at_place(a);
        let _ = std::panic::catch_unwind(|| loop { std::thread::park(); });
        // the run's end is no panic: this never comes
        flags(a)[1].fetch_add(1, Ordering::SeqCst);
    });
    spawn("allocating", move || {
        at_place(a);
        let mut keep: Vec<Vec<u8>> = Vec::new();
        loop {
            keep.push(vec![7u8; 4096]);
            if keep.len() > 256 { keep.clear(); }
        }
    });
    spawn("host-calls", move || {
        at_place(a);
        // natives taking the runtime's locks, as fast as they can go
        loop {
            std::thread::current().id();
            let _ = std::thread::Builder::new().name("short".into()).spawn(|| {}).map(|h| h.join());
        }
    });
    at_place(a);
    loop { std::thread::park(); }
}
"#;

#[cfg(unix)]
const ENDED_RUN_THREADS: u64 = 11;
#[cfg(windows)]
const ENDED_RUN_THREADS: u64 = 10;

/// Until the fixture's threads are all at their place.
fn wait_at_place(domain: DomainId, flags: &[AtomicU64; 4]) {
    wait(|| {
        if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
        flags[0].load(Ordering::SeqCst) >= ENDED_RUN_THREADS
    });
}

/// The run ends under threads in every kind of place: spinning in JIT code, in the program's own
/// waits, in system calls, under a catch_unwind, in the allocator, in natives taking the runtime's
/// locks. All are gone at once, no destructor of the program ran, its memory is back.
#[test]
fn a_run_ends_with_every_thread_wherever_it_is() {
    isolated_test!(a_run_ends_with_every_thread_wherever_it_is);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(ENDED_RUN, "fixture");
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait_at_place(domain, &flags);
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);
    let dead = info(domain).unwrap();
    assert_eq!((dead.threads, dead.base, dead.arena_committed), (0, 0, 0));
    assert_eq!(dead.state, DomainState::Stopped);
    assert_eq!(flags[1].load(Ordering::SeqCst), 0, "code of the ended run ran");
    assert_eq!(end.called_in, 0);
    // no native frame was left behind on the way: nothing of the runtime's can stay locked
    assert_eq!(end.abandoned, Vec::new());
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

/// The domain's main is in the native call with a first-generation return address on its stack.
#[cfg(any(target_vendor = "apple", target_os = "linux"))]
const LAYOUT_RETURN: &str = r#"
pub struct Layout { pub value: u64 }
#[inline(never)]
pub fn old(p: *mut u64) {
    let fields = Layout { value: 7 };
    let native: extern "C" fn() = unsafe { std::mem::transmute(*p.add(2) as usize) };
    native();
    unsafe { *p.add(1) = fields.value; }
}
#[inline(never)]
pub fn live_neighbour(out: *mut u64) { unsafe { *out = 23; } }
"#;

#[cfg(any(target_vendor = "apple", target_os = "linux"))]
#[test]
fn a_layout_patch_returns_the_native_lock_before_the_stale_frame_parks() {
    isolated_test!(a_layout_patch_returns_the_native_lock_before_the_stale_frame_parks);
    // Each stage a held foreign lock could block has this bound; the one stage without it is
    // the stale report's, which only the disk and the owner can slow down.
    const BOUND: Duration = Duration::from_secs(2);
    const UNBOUNDED_REPORT: &str = "the report of the fault and its decision";
    extern "C" fn native() {
        crate::os::thread::set_own_name("old-frame");
        native_holds_a_host_lock();
    }
    // Both the driver and its deadline observer belong to the host. They must run while the
    // fault pauses the program, including when allocating or starting a thread is what stalled.
    crate::os::thread::set_own_name("run-test-clock");
    let (stage, progress) = std::sync::mpsc::channel();
    let driver = std::thread::Builder::new().name("run-test-driver".into()).spawn(move || {
        std::env::set_var("RELAX_STALE_PARK", "1");
        crate::borrowck::lazy::disable_gandalf();
        let (mut u, entry, root) = fixture_of(LAYOUT_RETURN, "old");
        let up = &mut *u as *mut Unit;
        let def = |name| crate::program::DefId(u.prog.defs.iter().rposition(|d| u.prog.syms.str(d.name) == name).unwrap() as u32);
        let (old_def, live_def) = (def("old"), def("live_neighbour"));
        let file = u.prog.def(old_def).file;
        let old = u.fn_id(crate::jit::FnKey::Inst(old_def, Vec::new()));
        let live = u.fn_id(crate::jit::FnKey::Inst(live_def, Vec::new()));
        let neighbour = u.entry(live);
        let call_neighbour = || {
            let mut n = 0u64;
            unsafe { crate::jit::call_jit(up, neighbour, &mut n as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
            n
        };
        assert_eq!(call_neighbour(), 23);
        let live_code = u.fns[live as usize].code;
        let flags: [AtomicU64; 4] = Default::default();
        flags[2].store(native as *const () as usize as u64, Ordering::SeqCst);
        let (domain, _, main) = launch(&mut u, entry, None, &flags);
        wait(|| IN_NATIVE.load(Ordering::SeqCst) != 0);
        let tid = crate::inspect::threads::list().into_iter().find(|t| t.name == "old-frame").expect("the named thread holds the native lock").os_id;
        assert!(HOST_LOCK.try_lock().is_err());
        let first = u.fns[old as usize].code;
        let src = LAYOUT_RETURN.replace("pub value: u64 }", "pub extra: u64, pub value: u64 }").replace("value: 7 }", "extra: 9, value: 7 }");
        let edit = crate::reload::Edit { file, path: root.join("fixture.rs").to_string_lossy().into_owned(), src: src.into_bytes() };
        crate::invalid::FRAME_CHECK_MISSES.store(tid, Ordering::Relaxed);
        let patch = unsafe { crate::reload::apply(up, vec![edit]) };
        crate::invalid::FRAME_CHECK_MISSES.store(0, Ordering::Relaxed);
        assert_eq!(patch.rung, crate::reload::Rung::Patch, "{:?}", patch.lines);
        assert!(patch.lines.iter().any(|l| l.contains("parks when it returns there")), "the install found the old frame: {:?}", patch.lines);
        stage.send("old's first code is dead").unwrap();
        assert!(crate::stale::lookup(first).is_some(), "old's first code is dead");
        stage.send("a live neighbour runs on").unwrap();
        assert_eq!(u.fns[live as usize].code, live_code, "the neighbour was not rebuilt");
        assert_eq!(call_neighbour(), 23, "a live neighbour runs on");
        stage.send("no thread is stopped where it did not choose").unwrap();
        let held = crate::inspect::instrument::held_now(tid);
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        let held = held || crate::inspect::threads::is_held(tid);
        stage.send("the lock came back").unwrap();
        NATIVE_GO.store(1, Ordering::SeqCst);
        drop(HOST_LOCK.lock().unwrap());
        assert!(!held, "no thread is stopped where it did not choose");
        stage.send("it parked by its fault at the return").unwrap();
        // the fault side's own record, signal-safe stores before any output
        wait(|| crate::stale::recorded().iter().any(|(t, name)| *t == tid && name.ends_with("::old")) && crate::stale::stuck_threads() == 1);
        assert_eq!(flags[1].load(Ordering::SeqCst), 0, "the old layout read never completed");
        // The watcher's report writes stderr and the flight recorder's file and asks the owner: a
        // loaded box's disk held those writes past the bound (11-25: btrfs space reservation, the
        // owner idle), so it is waited for as the file's other sync points are.
        stage.send(UNBOUNDED_REPORT).unwrap();
        wait(|| crate::stale::verdicts().iter().any(|v| v.starts_with("stale access:") && v.contains("thread `old-frame`") && v.contains("::old`") && v.contains("its thread is parked")));
        // The decision exists before teardown starts. Seeing the verdict alone races pause_program.
        wait(|| crate::inspect::events::parked_now().iter().any(|p| p.1 == tid));
        stage.send("the allocator and thread creation are free").unwrap();
        let block = vec![23u8; 1024 * 1024];
        assert_eq!(std::thread::Builder::new().name("run-alloc-test".into()).spawn(move || std::hint::black_box(block)[1024 * 1024 - 1]).unwrap().join().unwrap(), 23);
        stage.send("it leaves with its run").unwrap();
        // The run's end as run_end::end begins it: the parked decision ends with the run (its
        // pause holds the process's main thread, a program thread) and the domain's end takes
        // the parked thread.
        crate::run_threads::run_ends();
        crate::inspect::events::run_ends();
        let end = end_run(up, BOUND);
        assert!(end.left.is_empty(), "it leaves with its run: {:?}", end.left);
        main.join().unwrap();
        let dead = info(domain).unwrap();
        assert_eq!((dead.threads, dead.base), (0, 0), "it leaves with its run");
        assert_eq!(dead.state, DomainState::Stopped);
        wait(|| !crate::inspect::instrument::paused());
        crate::run_threads::begin_run();
        drop(stage);
        domains().retain(|d| d.info.id != domain);
        drop(u);
        std::fs::remove_dir_all(root).unwrap();
    }).unwrap();
    if let Ok(mut checking) = progress.recv() {
        loop {
            let next = if checking == UNBOUNDED_REPORT { progress.recv().map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected) } else { progress.recv_timeout(BOUND) };
            match next {
                Ok(next) => checking = next,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("{checking}: exceeded {BOUND:?}"),
            }
        }
    }
    if let Err(panic) = driver.join() { std::panic::resume_unwind(panic); }
}

/// The domain's main waits in libc's condvar through a native call (flags[2]), as a library's
/// thread does; `reads` uses the layout the patch changes.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const LAYOUT_LIBC_WAIT: &str = r#"
pub struct Layout { pub value: u64 }
#[inline(never)]
pub fn reads(out: *mut u64) {
    let fields = Layout { value: 7 };
    unsafe { *out = fields.value; }
}
#[inline(never)]
pub fn waits(p: *mut u64) {
    let native: extern "C" fn() = unsafe { std::mem::transmute(*p.add(2) as usize) };
    native();
    unsafe { *p.add(1) = 1; }
}
"#;

/// A pthread_mutex_t (40 bytes) and, at 64, a pthread_cond_t (48): glibc's initializers are zeros.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
static LIBC_WAIT: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
static LIBC_WAIT_GO: AtomicU64 = AtomicU64::new(0);

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
extern "C" {
    fn pthread_mutex_lock(m: *mut u64) -> i32;
    fn pthread_mutex_unlock(m: *mut u64) -> i32;
    fn pthread_cond_wait(c: *mut u64, m: *mut u64) -> i32;
    fn pthread_cond_signal(c: *mut u64) -> i32;
}

/// A native op that waits in libc's condvar until LIBC_WAIT_GO.
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
extern "C" fn native_waits_in_libc() {
    crate::os::thread::set_own_name("in-libc-wait");
    let lock = LIBC_WAIT.as_ptr() as *mut u64;
    unsafe {
        pthread_mutex_lock(lock);
        while LIBC_WAIT_GO.load(Ordering::SeqCst) == 0 {
            pthread_cond_wait(lock.add(8), lock);
        }
        pthread_mutex_unlock(lock);
    }
}

/// A layout patch reads every program thread's stack before it installs (no frame may run code
/// built for the old layout). A thread parked in libc's condvar wait (glibc built with frame
/// pointers, whose early returns remember and restore the CFI state) is walked through libc's
/// frames to its program frames, and the patch installs. On a Linux box the walk stopped at
/// pthread_cond_wait, and every layout edit of an app with such a thread was a restart (11-24).
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn a_layout_patch_reads_a_thread_parked_in_a_libc_condvar_wait() {
    isolated_test!(a_layout_patch_reads_a_thread_parked_in_a_libc_condvar_wait);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(LAYOUT_LIBC_WAIT, "waits");
    let up = &mut *u as *mut Unit;
    let reads_def = crate::program::DefId(u.prog.defs.iter().rposition(|d| u.prog.syms.str(d.name) == "reads").unwrap() as u32);
    let file = u.prog.def(reads_def).file;
    let reads = u.fn_id(crate::jit::FnKey::Inst(reads_def, Vec::new()));
    let reads_entry = u.entry(reads);
    let read = || {
        let mut n = 0u64;
        unsafe { crate::jit::call_jit(up, reads_entry, &mut n as *mut u64 as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
        n
    };
    assert_eq!(read(), 7);
    let first = u.fns[reads as usize].code;
    let flags: [AtomicU64; 4] = Default::default();
    flags[2].store(native_waits_in_libc as *const () as usize as u64, Ordering::SeqCst);
    let (domain, _, main) = launch(&mut u, entry, None, &flags);
    // until it blocks in the condvar's futex (syscall 202; its lock is not contended)
    wait(|| {
        crate::inspect::threads::list().into_iter().find(|t| t.name == "in-libc-wait").is_some_and(|t| {
            std::fs::read_to_string(format!("/proc/self/task/{}/syscall", t.os_id)).is_ok_and(|s| s.split_whitespace().next() == Some("202"))
        })
    });
    let src = LAYOUT_LIBC_WAIT.replace("pub value: u64 }", "pub extra: u64, pub value: u64 }").replace("value: 7 }", "extra: 9, value: 7 }");
    let edit = crate::reload::Edit { file, path: root.join("fixture.rs").to_string_lossy().into_owned(), src: src.into_bytes() };
    let patch = unsafe { crate::reload::apply(up, vec![edit]) };
    // the wait ends before any check, so a failed one leaves no thread behind
    let lock = LIBC_WAIT.as_ptr() as *mut u64;
    unsafe {
        pthread_mutex_lock(lock);
        LIBC_WAIT_GO.store(1, Ordering::SeqCst);
        pthread_cond_signal(lock.add(8));
        pthread_mutex_unlock(lock);
    }
    main.join().unwrap();
    assert_eq!(patch.rung, crate::reload::Rung::Patch, "the layout patch installs: {:?}", patch.lines);
    assert_ne!(u.fns[reads as usize].code, first, "reads is rebuilt for the new layout");
    assert_eq!(read(), 7, "the new layout's code runs");
    assert_eq!(flags[1].load(Ordering::SeqCst), 1, "the thread went on from its wait");
    wait_thread_done(domain);
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// A thread of the run in a native call (flags[2]), and the callback flags[3] hands native code.
const NATIVE_CROSSINGS: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
static FLAGS: AtomicU64 = AtomicU64::new(0);
fn flags(a: usize) -> &'static [AtomicU64; 4] { unsafe { &*(a as *const [AtomicU64; 4]) } }
extern "C" fn callback() {
    flags(FLAGS.load(Ordering::SeqCst) as usize)[0].fetch_add(1, Ordering::SeqCst);
    loop { std::thread::park(); }
}
fn fixture(p: *mut u64) {
    let a = p as usize;
    FLAGS.store(a as u64, Ordering::SeqCst);
    // lazily compiled here, not in the native caller's call
    let c: extern "C" fn() = callback;
    std::thread::Builder::new().name("in-native".into()).spawn(move || {
        let native: extern "C" fn() = unsafe { std::mem::transmute(flags(a)[2].load(Ordering::SeqCst) as usize) };
        native();
        // the run ended meanwhile: this never comes
        flags(a)[1].fetch_add(1, Ordering::SeqCst);
    }).unwrap();
    flags(a)[3].store(c as usize as u64, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

const HOST_CALLBACK: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
static FLAGS: AtomicU64 = AtomicU64::new(0);
fn flags(a: usize) -> &'static [AtomicU64; 4] { unsafe { &*(a as *const [AtomicU64; 4]) } }
/// An app command's answer: it waits for the program's event loop, which never answers (a
/// `status` waits up to 600 s).
extern "C" fn query() {
    let a = FLAGS.load(Ordering::SeqCst) as usize;
    let (tx, rx) = std::sync::mpsc::channel::<u32>();
    // (its wait compiled before the call is counted as in it)
    let _ = rx.recv_timeout(std::time::Duration::from_millis(1));
    flags(a)[0].fetch_add(1, Ordering::SeqCst);
    let _ = rx.recv_timeout(std::time::Duration::from_secs(600));
    drop(tx);
    flags(a)[1].fetch_add(1, Ordering::SeqCst);
}
/// The same, through a native function of the host's (`native`) that the run ends in.
extern "C" fn query_through(native: extern "C" fn()) {
    let a = FLAGS.load(Ordering::SeqCst) as usize;
    flags(a)[0].fetch_add(1, Ordering::SeqCst);
    native();
    flags(a)[1].fetch_add(1, Ordering::SeqCst);
}
fn fixture(p: *mut u64) {
    let a = p as usize;
    FLAGS.store(a as u64, Ordering::SeqCst);
    let through: extern "C" fn(extern "C" fn()) = query_through;
    flags(a)[2].store(through as usize as u64, Ordering::SeqCst);
    let q: extern "C" fn() = query;
    flags(a)[3].store(q as usize as u64, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

/// A native caller that goes on after its call into a run: it waits in a read of a pipe nobody
/// writes, which a kick (what the next run's end sends it) ends. It tells its handle and the pipe's
/// other end, and answers whether a kick ended the read.
#[cfg(unix)]
fn takes_a_kick(told: std::sync::mpsc::Sender<(u64, i32)>) -> bool {
    extern "C" {
        fn pipe(fds: *mut i32) -> i32;
        fn read(fd: i32, buf: *mut core::ffi::c_void, n: usize) -> isize;
    }
    let mut fds = [0i32; 2];
    assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
    told.send((crate::os::thread::current(), fds[1])).unwrap();
    let mut b = [0u8; 1];
    let n = unsafe { read(fds[0], b.as_mut_ptr().cast(), 1) };
    // (EINTR)
    n == -1 && std::io::Error::last_os_error().raw_os_error() == Some(4)
}

/// Kicks the caller `takes_a_kick` told of until it has gone; one no kick reaches is let go by a
/// write to its pipe after 10 s (its answer says so).
#[cfg(unix)]
fn kick_until_gone<T>(caller: &std::thread::JoinHandle<T>, told: std::sync::mpsc::Receiver<(u64, i32)>) {
    extern "C" {
        fn write(fd: i32, buf: *const core::ffi::c_void, n: usize) -> isize;
    }
    let (t, w) = told.recv().unwrap();
    let start = std::time::Instant::now();
    while !caller.is_finished() {
        if start.elapsed() > Duration::from_secs(10) {
            unsafe { write(w, b"x".as_ptr().cast(), 1) };
            break;
        }
        crate::os::thread::kick(t);
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// In a call into the run as the run ends: the end's kick (SIGEMT) and a SIGBUS pending together,
/// as when a fetch of the ended code faults while a kick is on its way. Darwin sets the kick up
/// first and delivers the SIGBUS at its trampoline's entry; the call leaves from there.
#[cfg(target_vendor = "apple")]
extern "C" fn faults_over_a_kick() {
    extern "C" {
        fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32;
        fn sigpending(set: *mut u32) -> i32;
        fn pthread_kill(t: usize, sig: i32) -> i32;
        fn pthread_self() -> usize;
    }
    const SIG_BLOCK: i32 = 1;
    const SIG_UNBLOCK: i32 = 2;
    const SIGBUS: i32 = 10;
    let kick = 1u32 << (crate::os::fault::watchdog_signal() - 1);
    let mut both = [0u64; 16];
    both[0] = (kick | 1 << (SIGBUS - 1)) as u64;
    while !crate::domain::current_is_ending() {
        std::thread::sleep(Duration::from_millis(1));
    }
    unsafe {
        pthread_sigmask(SIG_BLOCK, &both, std::ptr::null_mut());
        // (the end kicks its threads until they have gone)
        let mut pending = 0u32;
        while sigpending(&mut pending) == 0 && pending & kick == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        pthread_kill(pthread_self(), SIGBUS);
        pthread_sigmask(SIG_UNBLOCK, &both, std::ptr::null_mut());
    }
}

/// Whether the calling thread is on its signal stack, as the kernel has it.
#[cfg(target_vendor = "apple")]
fn on_the_signal_stack() -> bool {
    #[repr(C)]
    struct StackT {
        sp: *mut u8,
        size: usize,
        flags: i32,
    }
    extern "C" {
        fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    }
    let mut old = StackT { sp: std::ptr::null_mut(), size: 0, flags: 0 };
    assert_eq!(unsafe { sigaltstack(std::ptr::null(), &mut old) }, 0);
    // (SS_ONSTACK)
    old.flags & 1 != 0
}

/// A host call into the program (an inspect app command, `Callback::call`) that waits in there when
/// the run ends comes back None at once: the end kicks its wait and the call leaves the ended code
/// for its recovery point. No code of the ended run runs after, the host's frames around the call
/// go on with what they hold (its lock is free), and the run's end is not held. Without the
/// recovery point the thread stayed in the ended code with an inspect request's run gate, and a
/// restart in place never finished (11-08).
///
/// The caller goes on after its call, and the next run's end still reaches it: a kick ends its
/// wait. On Darwin a second call leaves through a SIGBUS delivered over a kick the kernel had only
/// set up, which once left the caller with the kick's signal blocked and marked as on its signal
/// stack (11-21).
#[test]
fn a_host_call_into_the_program_ends_with_the_run() {
    isolated_test!(a_host_call_into_the_program_ends_with_the_run);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(HOST_CALLBACK, "fixture");
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, base, main) = launch(&mut u, entry, None, &flags);
    wait(|| flags[3].load(Ordering::SeqCst) != 0);
    let query = flags[3].load(Ordering::SeqCst) as usize;
    let scope = domains().iter().find(|d| d.info.base == base).map(|d| Callback { id: d.info.id, base, started: d.started }).unwrap();
    static CALLER_LOCK: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
    let (called, calls) = std::sync::mpsc::channel::<Option<()>>();
    let (told, tells) = std::sync::mpsc::channel::<(u64, i32)>();
    let caller = std::thread::spawn({
        let called = called.clone();
        move || {
            let answer = {
                let _g = CALLER_LOCK.lock().unwrap();
                let q: extern "C" fn() = unsafe { std::mem::transmute(query) };
                scope.call(|| q())
            };
            called.send(answer).unwrap();
            #[cfg(unix)]
            return takes_a_kick(told);
            #[cfg(not(unix))]
            return { drop(told); true };
        }
    });
    #[cfg(target_vendor = "apple")]
    let (nested_told, nested_tells) = std::sync::mpsc::channel::<(u64, i32)>();
    #[cfg(target_vendor = "apple")]
    let nested = std::thread::spawn({
        let through = flags[2].load(Ordering::SeqCst) as usize;
        let called = called.clone();
        move || {
            let q: extern "C" fn(extern "C" fn()) = unsafe { std::mem::transmute(through) };
            called.send(scope.call(|| q(faults_over_a_kick))).unwrap();
            (on_the_signal_stack(), takes_a_kick(nested_told))
        }
    });
    let callers = if cfg!(target_vendor = "apple") { 2 } else { 1 };
    wait(|| flags[0].load(Ordering::SeqCst) == callers);
    // (a grace far past a kick's round trip: a call that stays is named in `left`, not waited for)
    let end = end_run(up, Duration::from_secs(60));
    assert!(end.left.is_empty(), "the call left with the run: {:?}", end.left);
    for _ in 0..callers {
        assert!(calls.recv().unwrap().is_none(), "the call came back saying the run ended");
    }
    assert!(CALLER_LOCK.try_lock().is_ok());
    #[cfg(unix)]
    kick_until_gone(&caller, tells);
    #[cfg(not(unix))]
    drop(tells);
    assert!(caller.join().unwrap(), "the caller went on and a kick still reached it");
    #[cfg(target_vendor = "apple")]
    {
        kick_until_gone(&nested, nested_tells);
        let (on_stack, kicked) = nested.join().unwrap();
        assert!(!on_stack, "the call that left over a kick left its caller on the signal stack");
        assert!(kicked, "the call that left over a kick left its caller with kicks blocked");
    }
    assert_eq!(flags[1].load(Ordering::SeqCst), 0, "code of the ended run ran");
    assert!(scope.call(|| ()).is_none(), "a call after the run's end comes back at once");
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(CALLED_IN.load(Ordering::SeqCst), 0, "no call stayed in the ended run");
    domains().retain(|d| d.info.id != domain);
    std::mem::forget(u);
    std::fs::remove_dir_all(root).unwrap();
}

const HOST_QUERY: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
static FLAGS: AtomicU64 = AtomicU64::new(0);
fn flags(a: usize) -> &'static [AtomicU64; 4] { unsafe { &*(a as *const [AtomicU64; 4]) } }
/// A quick answer of an app command (a wake of the event loop), read through the run's statics.
extern "C" fn query() {
    let a = FLAGS.load(Ordering::SeqCst) as usize;
    flags(a)[0].fetch_add(1, Ordering::SeqCst);
}
fn fixture(p: *mut u64) {
    let a = p as usize;
    FLAGS.store(a as u64, Ordering::SeqCst);
    let q: extern "C" fn() = query;
    flags(a)[3].store(q as usize as u64, Ordering::SeqCst);
    while flags(a)[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
}
"#;

/// A host thread that stays (the inspect server's) calls into each run in turn: a `session
/// relaunch` wakes the program's event loop through `Callback::call` on it, run after run. Each call
/// runs in its run's domain, with its statics, and leaves nothing registered there. The recovery
/// point around the call once met the thread as new after the first run's end and freed the thread
/// block the call had just been given: the app died at its second relaunch (11-09).
#[test]
fn a_host_thread_calls_into_each_run_in_turn() {
    isolated_test!(a_host_thread_calls_into_each_run_in_turn);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (asks, asked) = std::sync::mpsc::channel::<(Callback, usize)>();
    let (answers, answered) = std::sync::mpsc::channel::<Option<()>>();
    let worker = std::thread::spawn(move || {
        for (scope, query) in asked {
            let q: extern "C" fn() = unsafe { std::mem::transmute(query) };
            answers.send(scope.call(|| q())).unwrap();
        }
    });
    let (mut units, mut roots) = (Vec::new(), Vec::new());
    for run in 0..3 {
        let (mut u, entry, root) = fixture_of(HOST_QUERY, "fixture");
        u.tier1 = None;
        let up = &mut *u as *mut Unit;
        let flags: [AtomicU64; 4] = Default::default();
        let (domain, base, main) = launch(&mut u, entry, None, &flags);
        wait(|| flags[3].load(Ordering::SeqCst) != 0);
        let scope = domains().iter().find(|d| d.info.base == base).map(|d| Callback { id: d.info.id, base, started: d.started }).unwrap();
        for k in 1..=2u64 {
            asks.send((scope, flags[3].load(Ordering::SeqCst) as usize)).unwrap();
            assert_eq!(answered.recv().unwrap(), Some(()), "run {run}: call {k} came back");
            assert_eq!(flags[0].load(Ordering::SeqCst), k, "run {run}: call {k} ran with its run's statics");
            let stayed = domains().iter().filter(|d| d.info.base == base).map(|d| d.threads.iter().filter(|t| t.name == "host callback").count()).sum::<usize>();
            assert_eq!(stayed, 0, "run {run}: call {k} left its registration in the domain");
        }
        flags[1].store(1, Ordering::SeqCst);
        main.join().unwrap();
        wait_thread_done(domain);
        let end = end_run(up, Duration::from_secs(10));
        assert!(end.left.is_empty(), "left: {:?}", end.left);
        crate::jit::thread_locals_end();
        unsafe { crate::os::tls::reset_statics() };
        domains().retain(|d| d.info.id != domain);
        units.push(u);
        roots.push(root);
    }
    drop(asks);
    worker.join().unwrap();
    std::mem::forget(units);
    // (each run's fixture is in the one folder of this process)
    roots.dedup();
    for root in roots {
        std::fs::remove_dir_all(root).unwrap();
    }
}

static HOST_LOCK: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
static IN_NATIVE: AtomicU64 = AtomicU64::new(0);
static NATIVE_GO: AtomicU64 = AtomicU64::new(0);

/// A native op of the host: takes a lock of the host's and stays in there for as long as a GPU call
/// or an allocator's slow path may.
extern "C" fn native_holds_a_host_lock() {
    let _g = HOST_LOCK.lock().unwrap();
    IN_NATIVE.store(1, Ordering::SeqCst);
    while NATIVE_GO.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A run ends with a thread inside a native call holding a lock of the host's, and native threads in
/// callbacks. Nothing native is cut off: the call finishes and lets go first (the end waits for it,
/// up to its grace, naming it), the thread ends as it comes back. A native caller of a callback gets
/// its call back, with its own lock free to go; so does a later call. One that bypasses the guard
/// stays where it is, counted on the ledger.
#[test]
fn a_run_ends_with_native_frames_in_it_and_no_host_lock_stays_held() {
    isolated_test!(a_run_ends_with_native_frames_in_it_and_no_host_lock_stays_held);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(NATIVE_CROSSINGS, "fixture");
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    flags[2].store(native_holds_a_host_lock as *const () as usize as u64, Ordering::SeqCst);
    let (domain, base, main) = launch(&mut u, entry, None, &flags);
    wait(|| flags[3].load(Ordering::SeqCst) != 0 && IN_NATIVE.load(Ordering::SeqCst) != 0);
    let callback = flags[3].load(Ordering::SeqCst) as usize;
    let scope = foreign_scope_of(base);
    assert_ne!(scope, 0);
    extern "C" fn thunk(p: *mut u8) {
        let f: extern "C" fn() = unsafe { std::mem::transmute(p) };
        f();
        // (no tail call: a frame of its own where the build keeps frame records, as a native
        // caller's function has)
        std::hint::black_box(p);
    }
    // a native thread with a lock of its own, in a callback that waits
    static CALLER_LOCK: std::sync::Mutex<u32> = std::sync::Mutex::new(0);
    let caller = std::thread::spawn(move || {
        let _g = CALLER_LOCK.lock().unwrap();
        let saved = foreign_enter(scope);
        let returned = crate::jit::app_call(thunk, callback as *mut u8);
        foreign_leave(saved);
        returned
    });
    wait(|| flags[0].load(Ordering::SeqCst) == 1);
    let end = end_run(up, Duration::from_millis(300));
    assert!(end.left.iter().any(|l| l.1 == "in-native"), "the call is never cut off: {:?}", end.left);
    assert!(!caller.join().unwrap(), "the callback's call came back saying the run ended");
    assert!(CALLER_LOCK.try_lock().is_ok());
    assert!(HOST_LOCK.try_lock().is_err(), "the native call is still in there");
    NATIVE_GO.store(1, Ordering::SeqCst);
    main.join().unwrap();
    wait_thread_done(domain);
    assert!(HOST_LOCK.try_lock().is_ok(), "and has let go");
    assert_eq!(flags[1].load(Ordering::SeqCst), 0, "code of the ended run ran");
    // a call that starts now comes back at once, on the same terms
    assert!(!std::thread::spawn(move || crate::jit::app_call(thunk, callback as *mut u8)).join().unwrap());
    assert_eq!(crate::jit::abandoned_frames(), Vec::new());
    assert_eq!(CALLED_IN.load(Ordering::SeqCst), 0);
    // no guard: nowhere to go, it stays and is counted
    let f: extern "C" fn() = unsafe { std::mem::transmute(callback) };
    std::thread::spawn(move || f());
    wait(|| CALLED_IN.load(Ordering::SeqCst) == 1);
    domains().retain(|d| d.info.id != domain);
    std::mem::forget(u);
    std::fs::remove_dir_all(root).unwrap();
}

extern "C" fn native_faults_at_the_end() {
    extern "C" {
        fn strlen(s: *const core::ffi::c_char) -> usize;
    }
    IN_NATIVE.store(1, Ordering::SeqCst);
    while NATIVE_GO.load(Ordering::SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    // (in the system's library: under the JIT this function is itself JIT code of the process; the
    // result is used, or an optimizer drops the call as having no effect)
    std::hint::black_box(unsafe { strlen(std::hint::black_box(8 as *const core::ffi::c_char)) });
}

/// A fault in a native call on a thread of a run that is ending is no crash of the process: the
/// thread ends and the frames it left are named. The process goes on.
#[test]
fn a_fault_while_a_run_ends_does_not_take_the_process_down() {
    isolated_test!(a_fault_while_a_run_ends_does_not_take_the_process_down);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(NATIVE_CROSSINGS, "fixture");
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    flags[2].store(native_faults_at_the_end as *const () as usize as u64, Ordering::SeqCst);
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait(|| flags[3].load(Ordering::SeqCst) != 0 && IN_NATIVE.load(Ordering::SeqCst) != 0);
    let end = end_run(up, Duration::from_millis(100));
    // (the run's other thread is on its way out, and under the JIT it may have code to compile first)
    assert!(end.left.iter().any(|l| l.1 == "in-native"), "{:?}", end.left);
    NATIVE_GO.store(1, Ordering::SeqCst);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(flags[1].load(Ordering::SeqCst), 0, "code of the ended run ran");
    let frames = crate::jit::abandoned_frames();
    assert!(!frames.is_empty(), "the native frame is named");
    domains().retain(|d| d.info.id != domain);
    std::mem::forget(u);
    std::fs::remove_dir_all(root).unwrap();
}

const NEXT_RUN: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    // allocation and threads in the next run: the locks the ended run's threads could have held
    let v = vec![3u8; 1 << 20];
    let h = std::thread::spawn(move || v.iter().map(|x| *x as u64).sum::<u64>());
    a[0].store(h.join().unwrap(), Ordering::SeqCst);
}
fn crashes(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    a[0].store(1, Ordering::SeqCst);
    unsafe { std::ptr::write_volatile(8 as *mut u64, 1) };
}
"#;

/// After ends under threads in every state, a next run in the process allocates and starts threads
/// at once: nothing the ended threads were in the middle of stays locked. A real crash in that run
/// is its crash, told from the ended run's faults by the address alone.
#[test]
fn the_next_run_finds_nothing_locked_and_its_crash_is_a_crash() {
    isolated_test!(the_next_run_finds_nothing_locked_and_its_crash_is_a_crash);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut old, entry, _root) = fixture_of(ENDED_RUN, "fixture");
    old.tier1 = None;
    let up = &mut *old as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, _base, main) = launch(&mut old, entry, None, &flags);
    wait_at_place(domain, &flags);
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);

    let (mut next, entry, _root) = fixture_of(NEXT_RUN, "fixture");
    next.tier1 = None;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, _base, main) = launch(&mut next, entry, None, &flags);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(flags[0].load(Ordering::SeqCst), 1 << 20 | 3 << 20);
    assert_eq!(info(domain).unwrap().state, DomainState::Exited(0));
    let (mut crash, entry, _root) = fixture_of(NEXT_RUN, "crashes");
    crash.tier1 = None;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, _base, main) = launch(&mut crash, entry, None, &flags);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(flags[0].load(Ordering::SeqCst), 1);
    assert!(matches!(info(domain).unwrap().state, DomainState::Crashed(_)), "{:?}", info(domain).unwrap().state);
}

// Holds one of each kind of thing a run gets from the OS, and never gives any of it back.
const HOLDS: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
extern "C" fn called_after_the_end() {}
// pthread_key_t: an unsigned long on Apple's systems, an unsigned int on the others
#[cfg(target_vendor = "apple")]
type Key = usize;
#[cfg(all(unix, not(target_vendor = "apple")))]
type Key = u32;
#[cfg(unix)]
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn signal(sig: i32, handler: usize) -> usize;
    fn pthread_key_create(key: *mut Key, dtor: usize) -> i32;
    fn pthread_atfork(prepare: Option<extern "C" fn()>, parent: Option<extern "C" fn()>, child: Option<extern "C" fn()>) -> i32;
}
#[cfg(unix)]
fn platform() -> *mut u8 {
    #[cfg(target_os = "linux")]
    const MAP_ANON: i32 = 0x20;
    #[cfg(not(target_os = "linux"))]
    const MAP_ANON: i32 = 0x1000;
    #[cfg(target_os = "linux")]
    const SIGUSR2: i32 = 12;
    #[cfg(not(target_os = "linux"))]
    const SIGUSR2: i32 = 31;
    unsafe {
        signal(SIGUSR2, called_after_the_end as usize);
        let mut key: Key = 0;
        pthread_key_create(&mut key, called_after_the_end as usize);
        pthread_atfork(Some(called_after_the_end), None, None);
        mmap(std::ptr::null_mut(), 1 << 20, 3, 2 | MAP_ANON, -1, 0)
    }
}
#[cfg(windows)]
extern "system" {
    fn VirtualAlloc(addr: *mut u8, len: usize, ty: u32, prot: u32) -> *mut u8;
    fn CreateEventW(attrs: *mut u8, manual: i32, initial: i32, name: *const u16) -> usize;
}
// one process left running, and one started to outlive the run
#[cfg(unix)]
fn children() -> (u32, u32) {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn setsid() -> i32;
    }
    let left = std::process::Command::new("sleep").arg("600").spawn().unwrap();
    let mut detached = std::process::Command::new("sleep");
    detached.arg("600");
    unsafe {
        detached.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    let detached = detached.spawn().unwrap();
    let pids = (left.id(), detached.id());
    std::mem::forget((left, detached));
    pids
}
#[cfg(windows)]
fn children() -> (u32, u32) {
    use std::os::windows::process::CommandExt;
    let waits = ["-n", "600", "127.0.0.1"];
    let left = std::process::Command::new("ping").args(waits).stdout(std::process::Stdio::null()).spawn().unwrap();
    let detached = std::process::Command::new("ping").args(waits).stdout(std::process::Stdio::null()).creation_flags(0x8).spawn().unwrap();
    let pids = (left.id(), detached.id());
    std::mem::forget((left, detached));
    pids
}
#[cfg(windows)]
fn platform() -> *mut u8 {
    unsafe {
        CreateEventW(std::ptr::null_mut(), 1, 0, std::ptr::null());
        VirtualAlloc(std::ptr::null_mut(), 1 << 20, 0x3000, 4)
    }
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    a[2].store(listener.local_addr().unwrap().port() as u64, Ordering::SeqCst);
    let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap();
    let dir = std::fs::read_dir(".").unwrap();
    let pipe = std::io::pipe().unwrap();
    let block = platform();
    unsafe { *block = 1 };
    std::mem::forget((listener, file, dir, pipe));
    let (left, detached) = children();
    a[1].store(left as u64, Ordering::SeqCst);
    a[3].store(detached as u64, Ordering::SeqCst);
    a[0].store(1, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

/// Whether process `pid` still runs (and is not just unreaped).
fn process_runs(pid: u32) -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use crate::os::win32::{CloseHandle, OpenProcess, WaitForSingleObject};
        unsafe {
            let h = OpenProcess(0x0010_0000, 0, pid);
            if h == 0 {
                return false;
            }
            let running = WaitForSingleObject(h, 0) != 0;
            CloseHandle(h);
            running
        }
    }
}

/// Ends a process the test started for good.
fn end_process(pid: u32) {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(pid as i32, 9) };
    }
    // (and waits for its end: a handle closed on a child that ended is when the runtime lets go
    // of its own)
    #[cfg(windows)]
    {
        use crate::os::win32::{CloseHandle, OpenProcess, TerminateProcess, WaitForSingleObject};
        unsafe {
            let h = OpenProcess(0x0001 | 0x0010_0000, 0, pid);
            TerminateProcess(h, 1);
            WaitForSingleObject(h, u32::MAX);
            CloseHandle(h);
        }
    }
}

/// A function the runtime this test runs under provides, found as a program finds it (through its
/// loader); None in a native run.
fn runtime_fn(name: &std::ffi::CStr) -> Option<usize> {
    #[cfg(unix)]
    let f = {
        // (as os/proc.rs declares it)
        extern "C" {
            fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
        }
        #[cfg(target_vendor = "apple")]
        let default = -2isize as *mut u8;
        #[cfg(not(target_vendor = "apple"))]
        let default = std::ptr::null_mut();
        unsafe { dlsym(default, name.as_ptr() as *const u8) as usize }
    };
    #[cfg(windows)]
    let f = {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(module: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        }
        unsafe { GetProcAddress(GetModuleHandleW(std::ptr::null()), name.as_ptr()) as usize }
    };
    (f != 0).then_some(f)
}

/// Under a runtime: from now on it records what this program holds (`relax_track_handles`).
fn track_held() {
    if let Some(f) = runtime_fn(c"relax_track_handles") {
        let track: extern "C" fn() = unsafe { std::mem::transmute(f) };
        track();
    }
}

/// What the runtime this test runs under records the program holding (`relax_held_handles`);
/// None with no runtime under the test.
fn held_by_the_program() -> Option<Vec<u64>> {
    let f = runtime_fn(c"relax_held_handles")?;
    let held: extern "C" fn(*mut u64, usize) -> usize = unsafe { std::mem::transmute(f) };
    let mut out = Vec::new();
    loop {
        let n = held(out.as_mut_ptr(), out.len());
        assert_ne!(n, usize::MAX, "the runtime tracks no handles of this program's");
        if n <= out.len() {
            out.truncate(n);
            return Some(out);
        }
        out.resize(n, 0);
    }
}

/// The path the Unix socket `fd` is bound to, as `getsockname` gives it (a connection a listener
/// accepted has the listener's). None for another descriptor or an unnamed socket.
#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn socket_path(fd: i32) -> Option<String> {
    extern "C" {
        fn getsockname(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
    }
    const AF_UNIX: u16 = 1;
    let mut addr = [0u8; 256];
    let mut len = addr.len() as u32;
    if unsafe { getsockname(fd, addr.as_mut_ptr(), &mut len) } != 0 {
        return None;
    }
    // sockaddr_un: its family (a u16, or on Apple's systems a length byte and a u8), then the path
    #[cfg(target_vendor = "apple")]
    let family = addr[1] as u16;
    #[cfg(not(target_vendor = "apple"))]
    let family = u16::from_ne_bytes([addr[0], addr[1]]);
    let path = addr.get(2..(len as usize).min(addr.len()))?;
    let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
    (family == AF_UNIX && end > 0).then(|| String::from_utf8_lossy(&path[..end]).into_owned())
}

/// What the process has open now: each descriptor and the file it names ("" for a pipe or a
/// socket where the system names none; a Unix socket with the path it is bound to), but the
/// listing's own; on Windows, each handle with its object type, a disk file with its path).
fn open_now() -> Vec<(u64, String)> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    return std::fs::read_dir("/proc/self/fd").unwrap().filter_map(|e| e.ok()).map(|e| {
        let fd = e.file_name().to_string_lossy().parse().unwrap_or(u64::MAX);
        let file = std::fs::read_link(e.path()).map(|t| t.to_string_lossy().into_owned()).unwrap_or_default();
        let bound = file.starts_with("socket:").then(|| socket_path(fd as i32)).flatten();
        (fd, bound.map_or(file.clone(), |path| format!("{file} {path}")))
    }).filter(|(_, file)| *file != format!("/proc/{}/fd", std::process::id())).collect();
    #[cfg(target_vendor = "apple")]
    {
        extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        const F_GETPATH: i32 = 50;
        return std::fs::read_dir("/dev/fd").unwrap().filter_map(|e| e.ok()).filter_map(|e| e.file_name().to_string_lossy().parse::<i32>().ok()).map(|fd| {
            let mut buf = [0u8; 1024];
            let named = unsafe { fcntl(fd, F_GETPATH, buf.as_mut_ptr()) } == 0;
            let end = buf.iter().position(|&b| b == 0).unwrap_or(0);
            (fd as u64, if named { String::from_utf8_lossy(&buf[..end]).into_owned() } else { socket_path(fd).unwrap_or_default() })
        }).filter(|(_, file)| file != "/dev/fd").collect();
    }
    // each handle by its value, named by its object type and, for a disk file, its path (a
    // pipe's name is never asked: that query can block)
    #[cfg(windows)]
    {
        use crate::os::win32::{GetCurrentProcess, GetProcessHandleCount};
        #[link(name = "kernel32")]
        extern "system" {
            fn GetHandleInformation(h: usize, flags: *mut u32) -> i32;
            fn GetFileType(h: usize) -> u32;
            fn GetFinalPathNameByHandleW(h: usize, path: *mut u16, len: u32, flags: u32) -> u32;
            fn GetProcessId(h: usize) -> u32;
            fn GetThreadId(h: usize) -> u32;
        }
        #[link(name = "ntdll")]
        extern "system" {
            fn NtQueryObject(h: usize, class: u32, info: *mut u8, len: u32, ret: *mut u32) -> i32;
        }
        const OBJECT_TYPE_INFORMATION: u32 = 2;
        const FILE_TYPE_DISK: u32 = 1;
        let describe = |h: usize| -> String {
            // OBJECT_TYPE_INFORMATION begins with the type's UNICODE_STRING (length in bytes, buffer)
            let mut info = [0u64; 128];
            let mut kind = String::new();
            if unsafe { NtQueryObject(h, OBJECT_TYPE_INFORMATION, info.as_mut_ptr() as *mut u8, 1024, std::ptr::null_mut()) } >= 0 {
                let len = (info[0] & 0xffff) as usize / 2;
                let buf = info[1] as *const u16;
                if !buf.is_null() {
                    kind = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(buf, len) });
                }
            }
            if kind == "File" && unsafe { GetFileType(h) } == FILE_TYPE_DISK {
                let mut path = [0u16; 1024];
                let n = unsafe { GetFinalPathNameByHandleW(h, path.as_mut_ptr(), path.len() as u32, 0) } as usize;
                if n > 0 && n < path.len() {
                    return format!("File {}", String::from_utf16_lossy(&path[..n]));
                }
            }
            match kind.as_str() {
                "Process" => format!("Process {}", unsafe { GetProcessId(h) }),
                "Thread" => format!("Thread {}", unsafe { GetThreadId(h) }),
                _ => kind,
            }
        };
        let mut n = 0;
        unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) };
        // handle values are multiples of 4: walk them until every counted handle is found, or
        // past a long run of unused values (the count can move while this walks)
        let (mut open, mut h, mut unused) = (Vec::new(), 4usize, 0u32);
        while open.len() < n as usize && unused < 1 << 16 {
            let mut flags = 0;
            if unsafe { GetHandleInformation(h, &mut flags) } != 0 {
                open.push((h as u64, describe(h)));
                unused = 0;
            } else {
                unused += 1;
            }
            h += 4;
        }
        open
    }
}

/// A file the compiler has open for itself: a source file a lazy compile read (kept for the rest
/// of the process), a cache store (any generation) or a store write in progress. Under load the
/// compiler's work for the first round lands in a later one, and so do these
fn the_compilers(file: &str) -> bool {
    let name = file.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(file);
    name.ends_with(".rs") || name.contains(".rcs")
}

/// A connection to this process's own remote endpoint: a node's devhub follows each JIT it finds
/// (its `observe` feed), dialing it at its next registry scan, and the endpoint keeps the accepted
/// socket (three descriptors: the carrier's reader, writer and shutdown) while the follower stays.
/// It is the process's, never the run's, and it comes in whichever round that scan does: in the
/// second round of a loaded box (11-14). A run's own sockets are no Unix socket bound there.
fn the_endpoints(file: &str) -> bool {
    file.ends_with(&format!("/makepad-remote-{}.sock", std::process::id()))
}

/// The run's end gives back what the run held, though it never did: the next run binds the same
/// port, and leaves nothing open. (A second run is the first a leak shows in: the first run's lazy
/// compiles open what stays open for the whole process.)
#[test]
fn a_run_ends_with_what_it_held_given_back() {
    isolated_test!(a_run_ends_with_what_it_held_given_back);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut counts, mut lefts) = (Vec::new(), Vec::new());
    track_held();
    for _ in 0..2 {
        let (mut u, entry, root) = fixture_of(HOLDS, "fixture");
        u.tier1 = None;
        let up = &mut *u as *mut Unit;
        let flags: [AtomicU64; 4] = Default::default();
        let open_before = open_now();
        let (domain, _base, main) = launch(&mut u, entry, None, &flags);
        wait(|| {
            if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
            flags[0].load(Ordering::SeqCst) == 1
        });
        let end = end_run(up, Duration::from_secs(10));
        assert!(end.left.is_empty(), "left: {:?}", end.left);
        main.join().unwrap();
        wait_thread_done(domain);
        let r = end.released;
        // the file, the directory, the pipe's two ends
        assert!(r.files >= 4, "{r:?}");
        assert_eq!(r.sockets, 1, "{r:?}");
        assert!(r.mappings >= 1 && r.mapped_bytes >= 1 << 20, "{r:?}");
        // the one it left running ended with it, and the one started to outlive it did not
        assert_eq!((r.children_ended, r.children_detached, r.children_left), (1, 1, 0), "{r:?}");
        assert!(!process_runs(flags[1].load(Ordering::SeqCst) as u32), "the run's child still runs");
        let detached = flags[3].load(Ordering::SeqCst) as u32;
        assert!(process_runs(detached), "the detached process did not stay");
        end_process(detached);
        #[cfg(unix)]
        assert_eq!(r.callbacks, 3, "{r:?}");
        let port = flags[2].load(Ordering::SeqCst) as u16;
        let again = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(again.is_ok(), "the run's port is still taken: {again:?}");
        drop(again);
        #[cfg(unix)]
        {
            #[cfg(target_os = "linux")]
            const SIGUSR2: i32 = 12;
            #[cfg(not(target_os = "linux"))]
            const SIGUSR2: i32 = 31;
            extern "C" {
                fn signal(sig: i32, handler: usize) -> usize;
            }
            assert_eq!(unsafe { signal(SIGUSR2, 0) }, 0, "the run's handler stayed");
        }
        // what the round left open, by what it names: not a count of the whole process. Under a
        // runtime (`makepad run` running this test), of that what the runtime records the program
        // holding: its own threads at work (a sleep's timer, a look at the process's threads)
        // hold handles for moments, and they are no one's leak. Natively nothing else runs here.
        let held = held_by_the_program();
        let left: Vec<_> = open_now().into_iter().filter(|f| !open_before.contains(f) && !the_compilers(&f.1) && !the_endpoints(&f.1) && held.as_ref().is_none_or(|h| h.contains(&f.0))).collect();
        counts.push(left.len());
        lefts.push(left);
        domains().retain(|d| d.info.id != domain);
        drop(u);
        std::fs::remove_dir_all(root).unwrap();
    }
    // (the first round's lazy compiles open what stays open for the whole process)
    assert_eq!(&counts[1..], &[0], "left open: {lefts:?}");
}

/// A child the main program starts leaves no handle of the runtime's behind. The runtime keeps
/// its own copy of a child's process handle, to end a run's children with their run; it made that
/// copy before asking whether a run records the child, and for the main program none does: the
/// copy was never closed, one process (and its pid) held per child for the life of the process.
#[cfg(windows)]
#[test]
fn a_main_programs_child_leaves_no_handle_behind() {
    isolated_test!(a_main_programs_child_leaves_no_handle_behind);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(MAIN_CHILD, "fixture");
    let out = [0u64; 1];
    // (on this thread, in no run: where the main program runs)
    unsafe { crate::jit::call_jit(&mut *u, entry, out.as_ptr() as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
    let child = format!("Process {}", out[0]);
    let held: Vec<_> = open_now().into_iter().filter(|(_, name)| *name == child).collect();
    assert!(held.is_empty(), "the ended child's process is still held: {held:?}");
    drop(u);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(windows)]
const MAIN_CHILD: &str = r#"
fn fixture(p: *mut u64) {
    let mut child = std::process::Command::new("cmd").args(["/c", "exit 0"]).spawn().unwrap();
    unsafe { *p = child.id() as u64 };
    child.wait().unwrap();
}
"#;

/// A handle the program closes through its copy (DuplicateHandle with DUPLICATE_CLOSE_SOURCE), or
/// through the native API's NtClose, is no longer the run's: the run's end closes nothing under its
/// number. Its record stayed, and the end closed whatever had that number by then, a handle of the
/// process's or of another run's.
#[cfg(windows)]
#[test]
fn a_handle_closed_through_its_copy_is_not_given_back() {
    isolated_test!(a_handle_closed_through_its_copy_is_not_given_back);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(CLOSED_THROUGH_ITS_COPY, "fixture");
    u.tier1 = None;
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let replaced = crate::domain_res::replaced_records();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait(|| {
        if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
        flags[0].load(Ordering::SeqCst) == 1
    });
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(end.released.files, 0, "the run's end closed a handle the program had closed: {:?}", end.released);
    // (a record left by a close the runtime did not see is replaced when the number comes back)
    assert_eq!(crate::domain_res::replaced_records() - replaced, 0, "a close went unseen, its record replaced");
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
const CLOSED_THROUGH_ITS_COPY: &str = r#"
use std::os::windows::io::IntoRawHandle;
use std::sync::atomic::{AtomicU64, Ordering};
extern "system" {
    fn GetCurrentProcess() -> isize;
    fn DuplicateHandle(sp: isize, s: isize, tp: isize, t: *mut isize, access: u32, inherit: i32, options: u32) -> i32;
    fn CloseHandle(h: isize) -> i32;
    fn NtClose(h: isize) -> i32;
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    // closed through the native API
    let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap().into_raw_handle() as isize;
    assert!(unsafe { NtClose(file) } >= 0);
    let file = std::fs::File::open(std::env::current_exe().unwrap()).unwrap().into_raw_handle() as isize;
    let me = unsafe { GetCurrentProcess() };
    let mut copy = 0;
    // DUPLICATE_CLOSE_SOURCE | DUPLICATE_SAME_ACCESS: the copy, and the file's first handle closed
    assert!(unsafe { DuplicateHandle(me, file, me, &mut copy, 0, 0, 1 | 2) } != 0);
    assert!(unsafe { CloseHandle(copy) } != 0);
    a[0].store(1, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

/// What a run opens through the native API (NtCreateFile, NtOpenFile, NtCreateNamedPipeFile) and
/// through kernel32's other creators (CreateFile2, ReOpenFile, OpenFileById) is the run's: its end
/// gives it back. The fixture makes a pipe as real std makes a child's, the pipe file system
/// opened once into a static (natively once per process; under a runtime each run's statics are
/// its own); a second run in the same process opens its own, pipes again, and gives it back too.
#[cfg(windows)]
#[test]
fn a_run_gives_back_what_the_native_api_opened() {
    isolated_test!(a_run_gives_back_what_the_native_api_opened);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    track_held();
    for round in 0..2 {
        let (mut u, entry, root) = fixture_of(NATIVE_API_OPENS, "fixture");
        u.tier1 = None;
        let up = &mut *u as *mut Unit;
        let flags: [AtomicU64; 4] = Default::default();
        let open_before = open_now();
        let (domain, _base, main) = launch(&mut u, entry, None, &flags);
        wait(|| {
            if let Some(DomainState::Crashed(why)) = state(domain) { panic!("round {round}: the fixture crashed: {why}"); }
            flags[0].load(Ordering::SeqCst) == 1
        });
        let held = flags[1].load(Ordering::SeqCst);
        let end = end_run(up, Duration::from_secs(10));
        assert!(end.left.is_empty(), "left: {:?}", end.left);
        main.join().unwrap();
        wait_thread_done(domain);
        assert_eq!(end.released.files as u64, held, "round {round}: the run's end gave back what it held: {:?}", end.released);
        // (the first round's lazy compiles open what stays open for the whole process)
        if round > 0 {
            let recorded = held_by_the_program();
            let left: Vec<_> = open_now().into_iter().filter(|f| !open_before.contains(f) && !the_compilers(&f.1) && !the_endpoints(&f.1) && recorded.as_ref().is_none_or(|h| h.contains(&f.0))).collect();
            assert!(left.is_empty(), "round {round} left open: {left:?}");
        }
        domains().retain(|d| d.info.id != domain);
        drop(u);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(windows)]
const NATIVE_API_OPENS: &str = r#"
use std::ptr::null;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering::SeqCst};
#[repr(C)]
struct UnicodeString { len: u16, max: u16, buf: *const u16 }
#[repr(C)]
struct ObjectAttributes { len: u32, root: isize, name: *const UnicodeString, attrs: u32, sd: *const u8, qos: *const u8 }
#[repr(C)]
#[derive(Default)]
struct IoStatus { status: usize, info: usize }
/// FILE_ID_DESCRIPTOR, of FileIdType
#[repr(C)]
struct FileId { size: u32, kind: i32, id: [u64; 2] }
extern "system" {
    fn NtOpenFile(h: *mut isize, access: u32, attrs: *const ObjectAttributes, io: *mut IoStatus, share: u32, options: u32) -> i32;
    fn NtCreateFile(h: *mut isize, access: u32, attrs: *const ObjectAttributes, io: *mut IoStatus, size: *const i64, file_attrs: u32, share: u32, disposition: u32, options: u32, ea: *const u8, ea_len: u32) -> i32;
    fn NtCreateNamedPipeFile(h: *mut isize, access: u32, attrs: *const ObjectAttributes, io: *mut IoStatus, share: u32, disposition: u32, options: u32, pipe_type: u32, read_mode: u32, completion: u32, instances: u32, inbound: u32, outbound: u32, timeout: *const i64) -> i32;
    fn CreateFile2(name: *const u16, access: u32, share: u32, disposition: u32, params: *const u8) -> isize;
    fn ReOpenFile(h: isize, access: u32, share: u32, flags: u32) -> isize;
    fn OpenFileById(volume: isize, id: *const FileId, access: u32, share: u32, security: *const u8, flags: u32) -> isize;
    fn GetFileInformationByHandle(h: isize, info: *mut [u32; 13]) -> i32;
}
const SYNCHRONIZE: u32 = 0x0010_0000;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const SHARE_ALL: u32 = 7;
/// FILE_SYNCHRONOUS_IO_NONALERT, FILE_NON_DIRECTORY_FILE
const SYNC_IO: u32 = 0x20;
const NON_DIRECTORY: u32 = 0x40;
const INVALID: isize = -1;
/// the pipe file system, opened once into a static, as real std's child pipes keep it
static PIPE_FS: AtomicIsize = AtomicIsize::new(0);
fn counted(s: &[u16]) -> UnicodeString { UnicodeString { len: (s.len() * 2) as u16, max: (s.len() * 2) as u16, buf: s.as_ptr() } }
fn attrs(root: isize, name: &UnicodeString) -> ObjectAttributes {
    ObjectAttributes { len: std::mem::size_of::<ObjectAttributes>() as u32, root, name, attrs: 0, sd: null(), qos: null() }
}
fn pipe_fs() -> isize {
    let h = PIPE_FS.load(SeqCst);
    if h != 0 { return h; }
    let path: Vec<u16> = r"\Device\NamedPipe\".encode_utf16().collect();
    let (mut h, mut io) = (0, IoStatus::default());
    let st = unsafe { NtOpenFile(&mut h, SYNCHRONIZE | GENERIC_READ, &attrs(0, &counted(&path)), &mut io, 3, SYNC_IO) };
    assert!(st >= 0, "NtOpenFile of the pipe file system: {st:#x}");
    PIPE_FS.store(h, SeqCst);
    h
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let exe = std::env::current_exe().unwrap();
    let exe = exe.to_str().unwrap();
    let mut io = IoStatus::default();
    // a pipe as real std makes a child's: our end under the pipe file system (FILE_CREATE, one
    // instance, byte stream), theirs opened through ours
    let empty = UnicodeString { len: 0, max: 0, buf: null() };
    let timeout = -50i64 * 10_000;
    let mut ours = 0;
    let st = unsafe { NtCreateNamedPipeFile(&mut ours, SYNCHRONIZE | GENERIC_READ, &attrs(pipe_fs(), &empty), &mut io, 2, 2, 0, 0, 0, 0, 1, 4096, 4096, &timeout) };
    assert!(st >= 0, "NtCreateNamedPipeFile: {st:#x}");
    let mut theirs = 0;
    let st = unsafe { NtOpenFile(&mut theirs, SYNCHRONIZE | GENERIC_WRITE | FILE_READ_ATTRIBUTES, &attrs(ours, &empty), &mut io, 0, NON_DIRECTORY | SYNC_IO) };
    assert!(st >= 0, "NtOpenFile of the pipe's other end: {st:#x}");
    // the program's own file through the native API (FILE_OPEN; an NT path: \??\ and the DOS one)
    let nt: Vec<u16> = format!(r"\??\{}", exe.trim_start_matches(r"\\?\")).encode_utf16().collect();
    let mut file = 0;
    let st = unsafe { NtCreateFile(&mut file, SYNCHRONIZE | GENERIC_READ, &attrs(0, &counted(&nt)), &mut io, null(), 0, SHARE_ALL, 1, NON_DIRECTORY | SYNC_IO, null(), 0) };
    assert!(st >= 0, "NtCreateFile: {st:#x}");
    // and through kernel32's other creators (OPEN_EXISTING)
    let wide: Vec<u16> = exe.encode_utf16().chain([0]).collect();
    let second = unsafe { CreateFile2(wide.as_ptr(), GENERIC_READ, SHARE_ALL, 3, null()) };
    assert_ne!(second, INVALID, "CreateFile2");
    let third = unsafe { ReOpenFile(second, GENERIC_READ, SHARE_ALL, 0) };
    assert_ne!(third, INVALID, "ReOpenFile");
    let mut info = [0u32; 13];
    assert!(unsafe { GetFileInformationByHandle(second, &mut info) } != 0, "the file's id");
    let id = FileId { size: std::mem::size_of::<FileId>() as u32, kind: 0, id: [((info[11] as u64) << 32) | info[12] as u64, 0] };
    let fourth = unsafe { OpenFileById(second, &id, GENERIC_READ, SHARE_ALL, null(), 0) };
    assert_ne!(fourth, INVALID, "OpenFileById");
    // held: the pipe file system, the pipe's two ends, and four of the file
    a[1].store(7, SeqCst);
    a[0].store(1, SeqCst);
    loop { std::thread::park(); }
}
"#;

/// A number the system gives out again while the runtime still records it means its earlier
/// close went by a path the runtime does not see (here the C library's own close, inside
/// `fclose` or `_close`): the new record replaces the old one, the replacement is counted, and
/// the run's end closes the number once, for its new owner. With both records kept, the end
/// closed it twice, and whatever had the number by the second close lost it.
#[test]
fn a_number_given_out_again_replaces_its_record() {
    isolated_test!(a_number_given_out_again_replaces_its_record);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(GIVEN_OUT_AGAIN, "fixture");
    u.tier1 = None;
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let replaced = crate::domain_res::replaced_records();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait(|| {
        if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
        flags[0].load(Ordering::SeqCst) == 1
    });
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(crate::domain_res::replaced_records() - replaced, 1, "the number's earlier record was replaced, and counted");
    assert_eq!(end.released.files, 1, "the number's new owner, closed once: {:?}", end.released);
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

const GIVEN_OUT_AGAIN: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(unix)]
mod native {
    use std::os::fd::{AsRawFd, IntoRawFd};
    extern "C" {
        fn fdopen(fd: i32, mode: *const u8) -> *mut u8;
        fn fclose(f: *mut u8) -> i32;
    }
    pub fn number(f: &std::fs::File) -> u64 { f.as_raw_fd() as u64 }
    /// closed the way native code closes it: the C library's own close, inside fclose
    pub fn close(f: std::fs::File) { unsafe { fclose(fdopen(f.into_raw_fd(), b"r\0".as_ptr())) }; }
}
#[cfg(windows)]
mod native {
    use std::os::windows::io::{AsRawHandle, IntoRawHandle};
    extern "C" {
        fn _open_osfhandle(h: isize, flags: i32) -> i32;
        fn _close(fd: i32) -> i32;
    }
    pub fn number(f: &std::fs::File) -> u64 { f.as_raw_handle() as u64 }
    /// closed the way native code closes it: the C runtime's own close, inside _close
    pub fn close(f: std::fs::File) { unsafe { _close(_open_osfhandle(f.into_raw_handle() as isize, 0)) }; }
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let exe = std::env::current_exe().unwrap();
    let first = std::fs::File::open(&exe).unwrap();
    let n = native::number(&first);
    native::close(first);
    // opened again until the system gives that number out (on unix the next open: the lowest free fd)
    let mut others = Vec::new();
    let again = loop {
        let f = std::fs::File::open(&exe).unwrap();
        if native::number(&f) == n { break f; }
        others.push(f);
    };
    drop(others);
    std::mem::forget(again);
    a[0].store(1, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

/// A program's signal handler that opens and closes (both async-signal-safe) while the main flow
/// holds many records, the table growing under it: its opens take the table's spare room (an
/// insert in a handler never grows a map, which could wait on the allocator the handler
/// interrupted), none is left out, and the run's end gives back what each side still holds.
#[cfg(unix)]
#[test]
fn opens_in_a_signal_handler_are_recorded_and_given_back() {
    isolated_test!(opens_in_a_signal_handler_are_recorded_and_given_back);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(HANDLER_OPENS, "fixture");
    u.tier1 = None;
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let (made, left_out) = crate::domain_res::handler_records();
    let replaced = crate::domain_res::replaced_records();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait(|| {
        if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
        flags[0].load(Ordering::SeqCst) == 1
    });
    let (held, opens) = (flags[1].load(Ordering::SeqCst), flags[2].load(Ordering::SeqCst));
    let (made_now, left_out_now) = crate::domain_res::handler_records();
    assert!(held >= 100 && opens >= 50, "the main flow held {held}, the handler opened {opens}");
    assert_eq!(made_now - made, opens, "every open in the handler was made in it (its thread marked)");
    assert_eq!(left_out_now - left_out, 0, "an open in the handler found no spare room");
    assert_eq!(crate::domain_res::replaced_records() - replaced, 0, "a close in the handler was not seen");
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(end.released.files as u64, held + 1, "what the main flow held and the handler's last open: {:?}", end.released);
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
const HANDLER_OPENS: &str = r#"
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering};
extern "C" {
    fn open(path: *const u8, flags: i32, ...) -> i32;
    fn close(fd: i32) -> i32;
    fn signal(sig: i32, h: usize) -> usize;
    fn raise(sig: i32) -> i32;
    fn getrlimit(r: i32, l: *mut [u64; 2]) -> i32;
    fn setrlimit(r: i32, l: *const [u64; 2]) -> i32;
}
#[cfg(any(target_os = "linux", target_os = "android"))]
const SIGUSR1: i32 = 10;
#[cfg(target_vendor = "apple")]
const SIGUSR1: i32 = 30;
#[cfg(any(target_os = "linux", target_os = "android"))]
const NOFILE: i32 = 7;
#[cfg(target_vendor = "apple")]
const NOFILE: i32 = 8;
/// `want` descriptors, or what the soft limit has room for (raised toward the hard one first)
fn room(want: u64) -> u64 {
    let mut l = [0u64; 2];
    unsafe {
        getrlimit(NOFILE, &mut l);
        let up = [l[1].min(want + 256), l[1]];
        if up[0] > l[0] { setrlimit(NOFILE, &up); }
        getrlimit(NOFILE, &mut l);
    }
    want.min(l[0].saturating_sub(64))
}
static PATH: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
static LAST: AtomicI32 = AtomicI32::new(-1);
static OPENS: AtomicU64 = AtomicU64::new(0);
/// the program's handler: opens the file again and closes what its last run opened
extern "C" fn on_usr1(_: i32) {
    let fd = unsafe { open(PATH.load(Ordering::SeqCst), 0) };
    if fd < 0 { return; }
    OPENS.fetch_add(1, Ordering::SeqCst);
    let last = LAST.swap(fd, Ordering::SeqCst);
    if last >= 0 { unsafe { close(last) }; }
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let exe = std::env::current_exe().unwrap();
    let exe = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(exe.as_os_str())).unwrap();
    PATH.store(exe.into_raw() as *mut u8, Ordering::SeqCst);
    unsafe { signal(SIGUSR1, on_usr1 as *const () as usize) };
    let many = room(1000);
    let mut held = Vec::new();
    for i in 0..many {
        let fd = unsafe { open(PATH.load(Ordering::SeqCst), 0) };
        assert!(fd >= 0, "open {i} of {many}");
        held.push(fd);
        if i % 2 == 0 { unsafe { raise(SIGUSR1) }; }
    }
    a[1].store(held.len() as u64, Ordering::SeqCst);
    a[2].store(OPENS.load(Ordering::SeqCst), Ordering::SeqCst);
    a[0].store(1, Ordering::SeqCst);
    loop { std::thread::park(); }
}
"#;

/// A program's handler left by siglongjmp, once on the thread's own stack and once on an
/// alternate stack (SA_ONSTACK, as std's own guard handler is) mapped above the thread's, where a
/// plain stack-pointer compare would never clear the handler's mark: its open is made in a handler,
/// and the opens after the jump are the main flow's, the mark cleared.
#[cfg(unix)]
#[test]
fn a_handler_left_by_siglongjmp_leaves_its_thread_outside_handlers() {
    isolated_test!(a_handler_left_by_siglongjmp_leaves_its_thread_outside_handlers);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(LEFT_BY_JUMP, "fixture");
    u.tier1 = None;
    let up = &mut *u as *mut Unit;
    let flags: [AtomicU64; 4] = Default::default();
    let (made, left_out) = crate::domain_res::handler_records();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    wait(|| {
        if let Some(DomainState::Crashed(why)) = state(domain) { panic!("the fixture crashed: {why}"); }
        flags[0].load(Ordering::SeqCst) == 1
    });
    let (on_alt, after) = (flags[1].load(Ordering::SeqCst), flags[2].load(Ordering::SeqCst));
    let (made_now, left_out_now) = crate::domain_res::handler_records();
    assert_eq!(on_alt, 0b10, "the first handler ran on the thread's own stack, the second on the alternate one");
    assert_eq!(made_now - made, 2, "each handler's own open was made in it, and none of the {after} opens after its jump");
    assert_eq!(left_out_now - left_out, 0, "an open found no spare room");
    let end = end_run(up, Duration::from_secs(10));
    assert!(end.left.is_empty(), "left: {:?}", end.left);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(end.released.files as u64, 2 + after, "the handlers' opens and the main flow's: {:?}", end.released);
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
const LEFT_BY_JUMP: &str = r#"
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering::SeqCst};
use std::ptr::{null, null_mut};
#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
struct SigAction { handler: usize, mask: [u64; 16], flags: i32, pad: i32, restorer: usize }
#[cfg(target_vendor = "apple")]
#[repr(C)]
struct SigAction { handler: usize, mask: u32, flags: i32 }
#[cfg(any(target_os = "linux", target_os = "android"))]
#[repr(C)]
struct StackT { sp: *mut u8, flags: i32, pad: i32, size: usize }
#[cfg(target_vendor = "apple")]
#[repr(C)]
struct StackT { sp: *mut u8, size: usize, flags: i32 }
#[cfg(any(target_os = "linux", target_os = "android"))]
const SA_ONSTACK: i32 = 0x0800_0000;
#[cfg(target_vendor = "apple")]
const SA_ONSTACK: i32 = 0x1;
#[cfg(any(target_os = "linux", target_os = "android"))]
const MAP_PRIVATE_ANON: i32 = 0x22;
#[cfg(target_vendor = "apple")]
const MAP_PRIVATE_ANON: i32 = 0x1002;
#[cfg(any(target_os = "linux", target_os = "android"))]
const SIGUSR1: i32 = 10;
#[cfg(target_vendor = "apple")]
const SIGUSR1: i32 = 30;
const SS_ONSTACK: i32 = 1;
const STACK: usize = 1 << 18;
extern "C" {
    fn open(path: *const u8, flags: i32, ...) -> i32;
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
    fn raise(sig: i32) -> i32;
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, off: i64) -> *mut u8;
    fn munmap(addr: *mut u8, len: usize) -> i32;
    #[cfg_attr(target_os = "linux", link_name = "__sigsetjmp")]
    fn sigsetjmp(env: *mut u8, savemask: i32) -> i32;
    fn siglongjmp(env: *mut u8, val: i32) -> !;
}
#[repr(C, align(16))]
struct Jmp([u8; 1024]);
static mut JMP: Jmp = Jmp([0; 1024]);
static PATH: AtomicPtr<u8> = AtomicPtr::new(null_mut());
static PHASE: AtomicU64 = AtomicU64::new(0);
static ON_ALT: AtomicU64 = AtomicU64::new(0);
static OPENED: AtomicI32 = AtomicI32::new(-1);
fn jmp() -> *mut u8 { unsafe { std::ptr::addr_of_mut!(JMP.0).cast() } }
/// the program's handler: notes the stack it runs on, opens the file, and leaves by siglongjmp
extern "C" fn leave(_: i32) {
    let mut ss: StackT = unsafe { std::mem::zeroed() };
    unsafe { sigaltstack(null(), &mut ss) };
    if ss.flags & SS_ONSTACK != 0 { ON_ALT.fetch_or(1 << PHASE.load(SeqCst), SeqCst); }
    OPENED.store(unsafe { open(PATH.load(SeqCst), 0) }, SeqCst);
    unsafe { siglongjmp(jmp(), 1) }
}
/// raises SIGUSR1: true once its handler jumped back here
#[inline(never)]
fn left_by_jump() -> bool {
    if unsafe { sigsetjmp(jmp(), 1) } != 0 { return true; }
    unsafe { raise(SIGUSR1) };
    false
}
/// a stack mapped above the calling thread's
fn stack_above() -> *mut u8 {
    let mark = 0u8;
    let here = &mark as *const u8 as usize;
    for shift in 28..40 {
        let hint = (here + (1usize << shift)) & !(STACK - 1);
        let p = unsafe { mmap(hint as *mut u8, STACK, 3, MAP_PRIVATE_ANON, -1, 0) };
        if p as isize == -1 { continue; }
        if p as usize > here { return p; }
        unsafe { munmap(p, STACK) };
    }
    panic!("no stack could be mapped above the thread's");
}
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let exe = std::env::current_exe().unwrap();
    let exe = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(exe.as_os_str())).unwrap();
    PATH.store(exe.into_raw() as *mut u8, SeqCst);
    let mut after = 0u64;
    for phase in 0..2u64 {
        PHASE.store(phase, SeqCst);
        let mut act: SigAction = unsafe { std::mem::zeroed() };
        act.handler = leave as *const () as usize;
        act.flags = if phase == 1 { SA_ONSTACK } else { 0 };
        assert_eq!(unsafe { sigaction(SIGUSR1, &act, null_mut()) }, 0, "the handler's installation");
        let mut old: StackT = unsafe { std::mem::zeroed() };
        let mut alt = null_mut();
        if phase == 1 {
            alt = stack_above();
            let mut ss: StackT = unsafe { std::mem::zeroed() };
            ss.sp = alt;
            ss.size = STACK;
            assert_eq!(unsafe { sigaltstack(&ss, &mut old) }, 0, "the alternate stack");
        }
        assert!(left_by_jump(), "the handler left by siglongjmp");
        assert!(OPENED.load(SeqCst) >= 0, "the handler's open");
        // the main flow's opens, after the jump
        for _ in 0..4 {
            assert!(unsafe { open(PATH.load(SeqCst), 0) } >= 0, "an open after the jump");
            after += 1;
        }
        if phase == 1 {
            assert_eq!(unsafe { sigaltstack(&old, null_mut()) }, 0, "the thread's alternate stack back");
            unsafe { munmap(alt, STACK) };
        }
    }
    a[1].store(ON_ALT.load(SeqCst), SeqCst);
    a[2].store(after, SeqCst);
    a[0].store(1, SeqCst);
    loop { std::thread::park(); }
}
"#;

const NATIVE_FAULT: &str = r#"
use std::sync::atomic::{AtomicU64, Ordering};
extern "C" { fn strlen(s: *const u8) -> usize; }
fn fixture(p: *mut u64) {
    let a = unsafe { &*(p as *const [AtomicU64; 4]) };
    let deep = a[2].load(Ordering::SeqCst);
    if deep == 0 {
        unsafe { strlen(8 as *const u8) };
    } else {
        let native: extern "C" fn() = unsafe { std::mem::transmute(deep as usize) };
        native();
    }
    a[0].store(1, Ordering::SeqCst);
}
"#;

#[inline(never)]
extern "C" fn native_faults_a_call_down() {
    #[inline(never)]
    fn down() {
        unsafe { std::ptr::write_volatile(8 as *mut u64, 1) };
    }
    down();
    std::hint::black_box(());
}

/// A native call the program made faults, on the program stack, in the function it called: the
/// run's failure, as a fault in JIT code is. On Windows that takes the system's whole search, and
/// the search has to end on the program stack.
#[test]
fn a_fault_in_a_native_function_a_program_called_fails_the_run() {
    isolated_test!(a_fault_in_a_native_function_a_program_called_fails_the_run);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, _root) = fixture_of(NATIVE_FAULT, "fixture");
    u.tier1 = None;
    let flags: [AtomicU64; 4] = Default::default();
    let (domain, _base, main) = launch(&mut u, entry, None, &flags);
    main.join().unwrap();
    wait_thread_done(domain);
    assert_eq!(flags[0].load(Ordering::SeqCst), 0, "the program went on after the fault");
    assert!(matches!(info(domain).unwrap().state, DomainState::Crashed(_)), "{:?}", info(domain).unwrap().state);
    std::mem::forget(u);
}

/// A fault deeper in native code, on the program stack: no recovery point can be trusted past
/// those frames, so the process ends with a report and the fault's code, and at once. It never
/// stays up in the system's search or its error report, which nothing would come back from: a
/// process that is gone is one a restart can follow.
#[test]
fn a_fault_deeper_in_native_code_ends_the_process_at_once() {
    const CHILD: &str = "RELAX_NATIVE_FAULT_CHILD";
    const THIS: &str = "domain::teardown_tests::a_fault_deeper_in_native_code_ends_the_process_at_once";
    if std::env::var_os(CHILD).is_some() {
        // (no core file: writing one is most of the time this takes)
        #[cfg(unix)]
        unsafe {
            extern "C" {
                fn setrlimit(resource: i32, limit: *const [u64; 2]) -> i32;
            }
            const RLIMIT_CORE: i32 = 4;
            setrlimit(RLIMIT_CORE, &[0, 0]);
        }
        let (mut u, entry, _root) = fixture_of(NATIVE_FAULT, "fixture");
        u.tier1 = None;
        let flags: [AtomicU64; 4] = Default::default();
        flags[2].store(native_faults_a_call_down as *const () as usize as u64, Ordering::SeqCst);
        let (_domain, _base, main) = launch(&mut u, entry, None, &flags);
        main.join().unwrap();
        return;
    }
    let log = std::env::temp_dir().join(format!("relax-native-fault-{}.log", std::process::id()));
    // (the fixture's compile comes first; a process that stayed up after its fault holds the
    // test here, a hung test)
    let status = crate::test_child::status(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", THIS, "--test-threads=1", "--nocapture"])
            .env(CHILD, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap()),
    );
    let told = std::fs::read_to_string(&log).unwrap();
    std::fs::remove_file(&log).unwrap();
    assert!(told.contains("the process ends"), "{status:?}\n{told}");
    #[cfg(windows)]
    assert_eq!(status.code(), Some(0xc000_0005u32 as i32), "{told}");
    #[cfg(unix)]
    assert!(std::os::unix::process::ExitStatusExt::signal(&status).is_some(), "{status:?}\n{told}");
}

/// A run whose boot ended before its unit was in (its build failed) has no code and no program
/// threads: its end, for a restart asked meanwhile (`session relaunch`), ends nothing and reads no
/// unit (the end read through the missing unit and the process died, 11-07).
#[test]
fn a_run_with_no_unit_ends_with_nothing_to_end() {
    isolated_test!(a_run_with_no_unit_ends_with_nothing_to_end);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let end = end_run(std::ptr::null_mut(), Duration::from_millis(100));
    assert_eq!((end.threads, end.left.len()), (0, 0), "{}", end.line());
}
