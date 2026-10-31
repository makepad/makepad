// Timers only: no profiler. Each batch has 768 mesh vertices + 256 wave points.
// The 8 MiB megabuf lives behind Inst; only two 8 KiB slices cross the wrapper.
extern crate self as test_crate;
#[path = "milk_kernel.rs"]
mod milk_kernel;
use std::time::Instant;

pub fn tick() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

fn measure<F: FnMut(&[f64], &mut [f64], u32) -> (u64, u64)>(label: &str, mut f: F) -> f64 {
    let input = vec![0.125; 1024];
    let mut output = vec![0.0; 1024];
    // Compare every output with the host source before timing (fresh state in each lane).
    let mut reference = milk_kernel::new_state();
    let mut expected = vec![0.0; 1024];
    for _ in 0..16 {
        milk_kernel::kernel(&mut reference, &input, &mut expected, 1);
        f(&input, &mut output, 1);
        for k in 0..1024 { assert!((output[k] - expected[k]).abs() < 1e-12); }
    }
    // Warm lazy compilation and tier counters, then allow the worker to finish.
    for _ in 0..4000 {
        milk_kernel::batch(&mut reference, &input, &mut expected, 1);
        f(&input, &mut output, 1);
    }
    std::thread::sleep(std::time::Duration::from_millis(1000));
    let mut full = 0.0;
    for mode in [0u32, 1, 2] {
        let mut total = Vec::new();
        let mut hosts = Vec::new();
        let mut host_bodies = Vec::new();
        let mut bodies = Vec::new();
        let mut guards = Vec::new();
        let mut wrappers = Vec::new();
        for _ in 0..1000 {
            // Adjacent host control: do not compare separate phases on a loaded runner.
            let ht = tick();
            let hb = milk_kernel::batch(&mut reference, &input, &mut expected, mode);
            hosts.push(tick() - ht);
            host_bodies.push(hb);
            let t = tick();
            let (b, inner) = f(&input, &mut output, mode);
            let elapsed = tick() - t;
            total.push(elapsed);
            // A real 20 ms deadline can expire on a descheduled runner. Count it
            // in total/p95 and report it; only completed bodies have a split timer.
            if b == u64::MAX { continue; }
            bodies.push(b);
            guards.push(if inner == 0 { 0 } else { elapsed.saturating_sub(inner) });
            wrappers.push((if inner == 0 { elapsed } else { inner }).saturating_sub(b));
        }
        total.sort();
        hosts.sort();
        host_bodies.sort();
        bodies.sort();
        guards.sort();
        wrappers.sort();
        assert!(!bodies.is_empty(), "gross regression: no completed batch");
        let mid = bodies.len() / 2;
        let ns = total[500] as f64;
        let body = bodies[mid] as f64;
        println!("milk {} mode={}: median={:.0} p95={} body={:.0} wrapper={} guard={} host={} host_body={} ratio={:.3} ns/batch calls=1000 completed={} slice_bytes=24576 megabuf_bytes=8388608", label, mode, ns, total[950], body, wrappers[mid], guards[mid], hosts[500], host_bodies[500], ns / hosts[500] as f64, bodies.len());
        if mode == 1 { full = ns / hosts[500] as f64; }
        assert!(output[0].is_finite());
    }
    full
}

#[test]
fn milk() {
    let mut state = milk_kernel::new_state();
    let _ = measure("host", |i, o, mode| (milk_kernel::batch(&mut state, i, o, mode), 0));
    let path = std::path::Path::new(file!()).parent().unwrap().join("milk_kernel.rs");
    let mut src = std::fs::read_to_string(&path).unwrap();
    let trusted = std::env::var("MAKEPAD_RUN_MODULES_TRUSTED").unwrap_or_default() == "1";
    if trusted {
        src.push_str("\npub fn make_handle() -> usize { Box::into_raw(new_state()) as usize }\npub fn call_handle(s: usize, i: &[f64], o: &mut [f64], mode: u32) -> u64 { batch(unsafe { &mut *(s as *mut MilkState) }, i, o, mode) }\npub fn drop_handle(s: usize) { unsafe { drop(Box::from_raw(s as *mut MilkState)); } }\n");
    }
    let m = std::relax::Module::load_source("milk_mod", "benchmark/milk_mod.rs", &src, &["test_crate"]).unwrap();
    let module;
    if trusted {
        let new: fn() -> usize = m.get("make_handle").unwrap();
        let call: fn(usize, &[f64], &mut [f64], u32) -> u64 = m.get("call_handle").unwrap();
        let destroy: fn(usize) = m.get("drop_handle").unwrap();
        let s = new();
        module = measure("trusted", |i, o, mode| (call(s, i, o, mode), 0));
        destroy(s);
    } else {
        let new: fn() -> std::relax::Inst = m.get("new_state").unwrap();
        let call: fn(std::relax::Inst, &[f64], &mut [f64], u32) -> u64 = m.get("batch").unwrap();
        let touch: fn(std::relax::Inst) -> f64 = m.get("touch").unwrap();
        let empty: fn() -> f64 = m.get("empty").unwrap();
        let s = new();
        module = measure("sandbox", |i, o, mode| (call(s, i, o, mode), 0));
        let s = new();
        let mut guard_calls = 0;
        let guarded = measure("guarded", |i, o, mode| {
            let mut inner = 0;
            let deadline = if guard_calls < 32 { 0 } else { 20 };
            guard_calls += 1;
            let body = std::relax::guard(deadline, || {
                let t = tick();
                let b = call(s, i, o, mode);
                inner = tick() - t;
                b
            });
            match body { Ok(b) => (b, inner), Err(_) => (u64::MAX, 0) }
        });
        let mut zero_boundary = Vec::new();
        for _ in 0..1000 {
            let t = tick();
            let b = call(s, &[], &mut [], 0);
            zero_boundary.push((tick() - t).saturating_sub(b));
        }
        zero_boundary.sort();
        println!("milk zero-slice wrapper={} ns/batch", zero_boundary[500]);
        for _ in 0..4000 { std::hint::black_box(empty()); std::hint::black_box(touch(s)); }
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let t = tick();
        let mut sum = 0.0;
        for _ in 0..4000 { sum += empty(); }
        let empty_ns = (tick() - t) as f64 / 4000.0;
        let t = tick();
        for _ in 0..4000 { sum += touch(s); }
        println!("milk inst: empty={:.0} touch={:.0} ns/call sum={}", empty_ns, (tick() - t) as f64 / 4000.0, sum);
        println!("milk guarded/host={:.3}", guarded);
    }
    println!("milk module/host={:.3}", module);
    m.unload();
    println!("out milk: true");
}
