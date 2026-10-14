use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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

fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition() {
        assert!(Instant::now() < deadline, "fixture did not reach its synchronization point");
        std::thread::yield_now();
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

#[test]
fn teardown_fixture_cycles_and_leaked_thread() {
    isolated_test!(teardown_fixture_cycles_and_leaked_thread);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture();
    let mut id = None;
    #[cfg(target_os = "linux")]
    let mut baseline = 0;
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
        let stop = std::thread::spawn(move || stop_for(domain, Duration::from_secs(30)).unwrap());
        wait(|| cancelled(domain));
        assert!(blocks().contains(&base));
        flags[1].store(1, Ordering::SeqCst);
        stop.join().unwrap();
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
            if cycle == 0 {
                baseline = now;
            }
            // Native allocator thread caches may vary a little; two MB leaked per cycle may not.
            assert!(now <= baseline + 256 * 1024, "cycle {cycle}: {now} > baseline {baseline}");
        }
        #[cfg(not(target_os = "linux"))]
        let _ = cycle;
    }

    // No cooperative completion: timeout must retain every allocation and the static block.
    let flags = [const { AtomicU64::new(0) }; 4];
    let (domain, base, main) = launch(&mut u, entry, id, &flags);
    main.join().unwrap();
    wait(|| flags[0].load(Ordering::SeqCst) != 0);
    let before = info(domain).unwrap();
    let freed = freed_by_teardown();
    stop_for(domain, Duration::ZERO).unwrap();
    let leaked = info(domain).unwrap();
    assert_eq!(leaked.state, DomainState::Leaked { threads: 1 });
    assert_eq!(leaked.arena_in_use, before.arena_in_use);
    assert_eq!(leaked.arena_committed, before.arena_committed);
    assert_eq!(leaked.base, base);
    assert_eq!(freed_by_teardown(), freed);
    assert!(prepare(&AppSpec {
        id: Some(domain),
        ..Default::default()
    })
    .is_err());
    // Release only for test-process hygiene. A timeout's retained memory stays retained even
    // after the thread subsequently ends; it must never be reclaimed by a delayed callback.
    flags[1].store(1, Ordering::SeqCst);
    wait(|| info(domain).unwrap().threads == 0);
    assert!(blocks().contains(&base));
    assert_eq!(freed_by_teardown(), freed);
    {
        let mut ds = domains();
        ds.iter_mut().find(|d| d.info.id == domain).unwrap().info.state = DomainState::Stopped;
    }
    teardown(domain);
    domains().retain(|d| d.info.id != domain);
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
        stop_for(id, Duration::ZERO).unwrap();
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

// Indicative cost of the program allocator: the same JIT workload on domain 0 (the process heap)
// and in a real app domain (its own arena heap). Timings on a busy machine are ratios only:
// `cargo test --release -p relax-compiler --lib domain_allocation_cost -- --ignored --nocapture`.
const ALLOC_FIXTURE: &str = r#"
use std::time::Instant;
fn churn(n: usize) -> u64 {
    let mut acc = 0u64;
    for i in 0..n { let b = Box::new(i as u64); acc = acc.wrapping_add(**std::hint::black_box(&b)); }
    acc
}
fn vecs(n: usize) -> u64 {
    let mut acc = 0u64;
    for i in 0..n { let mut v: Vec<u32> = Vec::new(); for j in 0..16 { v.push((i + j) as u32); } acc = acc.wrapping_add(v[15] as u64); }
    acc
}
fn batch(n: usize) -> u64 {
    let mut live: Vec<Vec<u8>> = (0..1000).map(|_| Vec::new()).collect();
    let mut acc = 0u64;
    for i in 0..n { let sz = 16 << (i % 7); live[i % 1000] = vec![i as u8; sz]; acc += live[(i * 7) % 1000].len() as u64; }
    acc
}
fn threads(f: fn() -> u64, n: usize) -> u64 {
    let hs: Vec<_> = (0..n).map(|_| std::thread::spawn(move || f())).collect();
    let mut acc = 0; for h in hs { acc ^= h.join().unwrap(); }
    acc
}
fn app_job() -> u64 {
    let mut acc = 0u64;
    for i in 0..10_000 {
        let strings: Vec<String> = (0..50).map(|_| String::from("layout label")).collect();
        let mut map = std::collections::HashMap::<u64, Vec<u8>>::new();
        for j in 0..100u64 { map.insert(j, vec![(i + j as usize) as u8; 32]); }
        std::hint::black_box(&strings);
        std::hint::black_box(&map);
        acc = acc.wrapping_add(strings[49].len() as u64 + map[&99][0] as u64);
    }
    acc
}
fn vecs_job() -> u64 { vecs(200_000) }
fn churn_job() -> u64 { churn(1_000_000) }
fn batch_job() -> u64 { batch(1_000_000) }
fn bench(out: *mut u64) {
    let out = unsafe { std::slice::from_raw_parts_mut(out, 8) };
    let t = Instant::now(); let a = churn(1_000_000); out[0] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let b = vecs(200_000); out[1] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let c = batch(1_000_000); out[2] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let d = threads(churn_job, 4); out[3] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let e = threads(batch_job, 4); out[4] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let f = threads(vecs_job, 4); out[5] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let g = app_job(); out[6] = t.elapsed().as_nanos() as u64;
    let t = Instant::now(); let h = threads(app_job, 4); out[7] = t.elapsed().as_nanos() as u64;
    assert!(a + b + c + d + e + f + g + h > 0);
}
"#;

#[test]
#[ignore]
fn domain_allocation_cost() {
    isolated_test!(domain_allocation_cost);
    let _serial = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut u, entry, root) = fixture_of(ALLOC_FIXTURE, "bench");
    let names = ["box 1e6", "vec16 2e5", "mixed live 1e6", "box 1e6 x4 threads", "mixed live 1e6 x4 threads", "vec16 2e5 x4 threads", "native app-like 1e4", "native app-like 1e4 x4 threads"];
    let up = &mut *u as *mut Unit;
    for round in 0..4 {
        let d0 = [const { AtomicU64::new(0) }; 8];
        unsafe { crate::jit::call_jit(up, entry, d0.as_ptr() as u64) }.unwrap_or_else(|p| panic!("{}", p.message));
        let d1 = [const { AtomicU64::new(0) }; 8];
        let (domain, _, main) = launch_with(&mut u, entry, None, d1.as_ptr() as u64);
        main.join().unwrap();
        wait(|| info(domain).unwrap().threads == 0);
        stop_for(domain, Duration::from_secs(30)).unwrap();
        for (i, n) in names.iter().enumerate() {
            let (a, b) = (d0[i].load(Ordering::SeqCst) as f64 / 1e6, d1[i].load(Ordering::SeqCst) as f64 / 1e6);
            println!("ALLOC round {round} {n}: domain0 {a:.1} ms, app domain {b:.1} ms, ratio {:.2}", b / a);
        }
    }
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
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
    stop_for(domain, Duration::from_secs(30)).unwrap();
    let dead = info(domain).unwrap();
    assert_eq!((dead.threads, dead.arena_in_use, dead.arena_committed), (0, 0, 0));
    domains().retain(|d| d.info.id != domain);
    drop(u);
    std::fs::remove_dir_all(root).unwrap();
}
