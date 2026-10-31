//! Direct calls through patched thunks (calls.DESIGN.md): a slot's code changes while other
//! threads keep calling it.
use std::sync::atomic::Ordering;
use std::sync::Arc;

const APP: &str = r#"
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
static STOP: AtomicBool = AtomicBool::new(false);
static SEEN: [AtomicU64; 3] = [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)];
static THREADS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());
#[inline(never)]
pub fn leaf() -> u64 { 1 }
pub fn start(_out: &mut u64) {
    let mut t = THREADS.lock().unwrap();
    for _ in 0..4 {
        t.push(std::thread::spawn(|| {
            while !STOP.load(Ordering::Relaxed) {
                let v = leaf();
                SEEN[if v == 1 { 0 } else if v == 2 { 1 } else { 2 }].fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    while SEEN[0].load(Ordering::Relaxed) < 10_000 { std::hint::spin_loop(); }
}
pub fn twos(out: &mut u64) {
    *out = SEEN[1].load(Ordering::Relaxed);
}
pub fn finish(out: &mut [u64; 3]) {
    let t0 = std::time::Instant::now();
    while SEEN[1].load(Ordering::Relaxed) < 100_000 && t0.elapsed().as_secs() < 30 { std::hint::spin_loop(); }
    STOP.store(true, Ordering::Relaxed);
    for h in THREADS.lock().unwrap().drain(..) { h.join().unwrap(); }
    for i in 0..3 { out[i] = SEEN[i].load(Ordering::Relaxed); }
}
"#;

/// The thunk head as it is (read through the executable view).
fn head(u: &crate::jit::Unit, id: u32) -> u64 {
    let at = u.slots.thunk(id);
    #[cfg(target_arch = "aarch64")]
    return unsafe { std::ptr::read_volatile(at as *const u32) } as u64;
    #[cfg(not(target_arch = "aarch64"))]
    return unsafe { std::ptr::read_volatile(at as *const u64) };
}

fn head_for(u: &crate::jit::Unit, id: u32, target: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    use relax_backend::arm64 as tgt;
    #[cfg(not(target_arch = "aarch64"))]
    use relax_backend::x64 as tgt;
    tgt::thunk_head(u.slots.thunk(id), target) as u64
}

#[test]
fn patched_callee_while_callers_run() {
    // process-global runtime state: a child process of this test binary
    const CHILD: &str = "MAKEPAD_RUN_CALLS_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "calls_tests::patched_callee_while_callers_run", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-calls-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), APP).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "calls_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (d_leaf, d_start, d_twos, d_finish) = (def("leaf"), def("start"), def("twos"), def("finish"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let (leaf, start, twos, finish) = crate::owner::call(crate::owner::Class::Program, || {
        let leaf = u.fn_id(crate::jit::FnKey::Inst(d_leaf, vec![]));
        let e = |d| { let id = u.fn_id(crate::jit::FnKey::Inst(d, vec![])); u.entry(id) };
        (leaf, e(d_start), e(d_twos), e(d_finish))
    });
    let call = |entry: u64, arg: u64| {
        unsafe { crate::jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    };
    let mut x = 0u64;
    call(start, &mut x as *mut u64 as u64);
    // the callers run; leaf has code, and its thunk jumps straight to it
    let old = u.fns[leaf as usize].code;
    assert!(u.fns[leaf as usize].compiled);
    assert_eq!(head(&u, leaf), head_for(&u, leaf, old), "the thunk head jumps to leaf's code");
    // a live patch replaces leaf while four threads keep calling it
    let old_file = u.prog.def(d_leaf).file;
    let text = APP.replace("pub fn leaf() -> u64 { 1 }", "pub fn leaf() -> u64 { 2 }");
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: old_file, path: app.clone(), src: text.into_bytes() }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
    // the patched code is built on its next call
    let t0 = std::time::Instant::now();
    loop {
        call(twos, &mut x as *mut u64 as u64);
        if x > 1000 || t0.elapsed().as_secs() > 30 { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let new = u.fns[leaf as usize].code;
    assert_ne!(old, new, "leaf has new code");
    // racing writers of the table word (old and new code: both are leaf), callers still running:
    // the head ends matching the word
    let slots = &*u.slots as *const relax_backend::rir::SlotTable as usize;
    let racers: Vec<_> = (0..2).map(|k| std::thread::spawn(move || {
        let slots = unsafe { &*(slots as *const relax_backend::rir::SlotTable) };
        let w = crate::jit::SlotWord::of(slots, leaf);
        for i in 0..20_000u32 {
            w.store(if (i + k) % 2 == 0 { old } else { new }, Ordering::SeqCst);
        }
    })).collect();
    for r in racers { r.join().unwrap(); }
    let word = crate::jit::SlotWord::of(&u.slots, leaf).load(Ordering::SeqCst);
    assert_eq!(head(&u, leaf), head_for(&u, leaf, word), "after racing stores the head matches the word");
    crate::jit::SlotWord::of(&u.slots, leaf).store(new, Ordering::SeqCst);
    assert_eq!(head(&u, leaf), head_for(&u, leaf, new));
    let mut seen = [0u64; 3];
    call(finish, seen.as_mut_ptr() as u64);
    assert_eq!(seen[2], 0, "a caller saw neither the old nor the new leaf: {seen:?}");
    assert!(seen[0] > 0 && seen[1] >= 100_000, "callers ran both versions: {seen:?}");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}
