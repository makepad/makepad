use super::*;
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
    let root = std::env::temp_dir().join(format!("relax-domain-teardown-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("fixture.rs");
    std::fs::write(&path, source).unwrap();
    // the std sources of this checkout (as lazy_reload_tests.rs does: a test binary is not next to them)
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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
/// Each of these is one-off process state that the cycles would otherwise create in whichever
/// cycle first needs it, which depends on how the threads interleave (and so on machine load):
/// lazy compiles of the fixture's code, and the symbol tables of the images a teardown stack
/// dump names (it runs only when a thread outlives its domain's stop).
#[cfg(target_os = "linux")]
fn warm_process_state(u: &mut Unit, entry: u64) {
    extern "C" {
        fn getpid() -> i32;
    }
    crate::jit::precompile_reachable(u, entry);
    assert!(crate::inspect::symbols::host_symbol(getpid as *const () as usize as u64).is_some());
    // This test's own code is in a host image only in a rustc build: under the JIT it is JIT
    // code, which no image's symbol table holds (a stack dump names it from the JIT's tables).
    #[cfg(not(relax))]
    assert!(crate::inspect::symbols::host_symbol(warm_process_state as *const () as usize as u64).is_some());
}

#[test]
fn teardown_fixture_cycles_and_retiring_thread() {
    isolated_test!(teardown_fixture_cycles_and_retiring_thread);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture();
    let mut id = None;
    #[cfg(target_os = "linux")]
    let mut baseline = 0;
    #[cfg(target_os = "linux")]
    warm_process_state(&mut u, entry);
    for cycle in 0..20 {
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
        {
            let now = heap_in_use();
            // Cycle 0 launches a new domain; from cycle 1 on the domain is cycled, which is the steady
            // state a leak shows against.
            if cycle == 1 {
                baseline = now;
            }
            // Native allocator thread caches may vary a little; two MB leaked per cycle may not.
            if cycle >= 1 {
                assert!(now <= baseline + 256 * 1024, "cycle {cycle}: {now} > baseline {baseline}");
            }
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cycle;
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
    domains().retain(|d| d.info.id != domain && d.info.id != replacement);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}

// Compile the production pool, CPU budget and stop_workers into the JIT fixture,
// with only their UI/logging/priority adapters replaced. No GPU, package resolver,
// external tools or copied implementation of the pool is involved.
fn static_pool_fixture() -> String {
    let budget = include_str!("../../../../platform/cpu_budget/src/lib.rs")
        .replace("use makepad_relax_api::relax_api;\n", "")
        .replace("#[relax_api]\n", "");
    let source = include_str!("../../../../platform/src/thread.rs");
    let network = source.split_once("pub use makepad_network::{").unwrap().1
        .split_once("};").unwrap().0;
    let runtime = source.replacen("pub mod ui_hang;", r#"
        pub mod ui_hang {
            pub struct UiPhase;
            pub struct UiPhaseGuard;
            pub struct UiIdleGuard;
            pub fn ui_phase() {}
            pub fn ui_event_phase() {}
            pub fn ui_idle() -> UiIdleGuard { UiIdleGuard }
            pub mod relax_timeline {
                pub struct Hooks {
                    pub begin: fn(*const u8, usize),
                    pub end: fn(*const u8, usize),
                }
                pub fn hooks() -> Option<&'static Hooks> { None }
            }
        }
    "#, 1).replacen(&format!("pub use makepad_network::{{{network}}};"), r#"
        pub struct SignalToUI;
        impl SignalToUI {
            pub fn set_ui_signal() {}
            pub fn set_internal_signal() {}
        }
    "#, 1);
    let restart = include_str!("../../../../platform/src/restart.rs");
    let stop = restart.split_once("pub(crate) fn stop_workers(").unwrap().1
        .split_once("\n}").unwrap().0;
    let adapters = r#"
        #[macro_export] macro_rules! log { ($($t:tt)*) => {} }
        #[macro_export] macro_rules! error { ($($t:tt)*) => {} }
        #[macro_export] macro_rules! trace { ($($t:tt)*) => {} }
        mod os { pub fn wake_ui_event_loop() {} }
        mod log { pub fn stop_sink() {} }
        mod remote { pub fn stop_encoder() {} }
        mod event {
            #[derive(Clone, Copy)] pub struct Timer(pub u64);
            pub struct TimerEvent { pub timer_id: u64 }
            pub enum Event { Shutdown, Timer(TimerEvent) }
        }
        mod cx_api {
            #[derive(Clone, Copy, Debug, Default)]
            pub enum CxThreadPriority { #[default] Utility, UserInitiated, UserInteractive }
        }
        mod cx {
            pub struct Cx {
                pub thread_spawner: crate::thread::ThreadSpawner,
                pub task_pool: std::cell::OnceCell<crate::thread::TaskPool>,
                pub cpu_cores: usize,
            }
            impl Cx {
                pub fn monotonic_now() -> f64 {
                    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
                    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
                }
                pub fn set_thread_priority(_: crate::cx_api::CxThreadPriority) -> crate::thread::PriorityStatus {
                    crate::thread::PriorityStatus::Applied
                }
                pub fn stop_timer(&mut self, _: crate::event::Timer) {}
                pub fn start_timeout(&mut self, _: f64) -> crate::event::Timer { crate::event::Timer(1) }
            }
        }
    "#;
    format!("{adapters}\nmod makepad_cpu_budget {{\n{budget}\n}}\nmod thread {{\n{runtime}\nuse crate::makepad_cpu_budget;\npub(crate) fn fixture_pool(spawner: ThreadSpawner) -> TaskPool {{ let pool = TaskPool::create(spawner, 3).unwrap(); while pool.inner.workers.iter().any(|worker| !worker.idle.load(Ordering::Acquire)) {{ std::thread::yield_now(); }} pool }}\n}}\nmod restart {{\nstatic STOP_WAKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);\npub(crate) fn stop_workers({stop}\n}}\n}}\n{STATIC_POOL_APP}")
}

const STATIC_POOL_APP: &str = r#"
use std::sync::{OnceLock, atomic::{AtomicU64, Ordering}};
use thread::{TaskPool, ThreadSpawner, Class};
static POOL: OnceLock<TaskPool> = OnceLock::new();
fn static_pool_app(p: *mut u64) {
    let flags = unsafe { &*(p as *const [AtomicU64; 4]) };
    assert!(POOL.get().is_none(), "relaunch must start from its own static image");
    let spawner = ThreadSpawner::for_current_thread(4);
    let pool = POOL.get_or_init(|| thread::fixture_pool(spawner.clone()));
    flags[0].store(1, Ordering::SeqCst);
    while flags[1].load(Ordering::SeqCst) == 0 { std::thread::yield_now(); }
    restart::stop_workers(&spawner);
    assert!(!pool.is_open());
    assert!(pool.submit(Class::Visible, || ()).is_err());
    assert!(POOL.get().is_some(), "the pool handle remains in the static");
    flags[2].store(1, Ordering::SeqCst);
}
"#;

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
            assert!(live.thread_names.contains(&format!("makepad-pool-{index}")));
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
    // a deadline only to report the deadlock (the threads stay stuck when it is there)
    match done_rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(Ok(true))) => {}
        Ok(r) => panic!("the build script did not run: {:?}", r.map(|r| r.map_err(|e| e.to_string()))),
        Err(_) => panic!("a process start under a loading domain's compile job never finished (fork waits for the loader's job)"),
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
const TLS_FIXTURE: &str = r#"
use std::cell::Cell;
thread_local! { static N: Cell<u64> = const { Cell::new(0) }; }
fn bump(k: u64) -> u64 {
    N.with(|n| { n.set(n.get() + k); n.get() })
}
fn tls_fixture(p: *mut u64) {
    let a = bump(3) + bump(4);
    let b = std::thread::spawn(|| bump(5) + bump(6)).join().unwrap();
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
    let out = [0u64; 2];
    unsafe { crate::jit::call_jit(&mut *u, entry, out.as_ptr() as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
    assert_eq!(out, [10 * 100 + 16, 7]);
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
