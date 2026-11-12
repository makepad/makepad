//! A Splash VM called over and over keeps a flat heap. Loop scopes do not pin the
//! scope they run in, a `for` over a range literal frees the range, and what does
//! escape is collected by the host's GC cadence (the event loop's
//! `needs_gc` -> `gc`). Before, every loop run left its call scope (or outer
//! iteration scope) and its range object behind, and a long-running eval loop
//! filled the 2^24-slot object table ("GenVec<ScriptObjectData> is full").

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm
}

/// Evaluates `code` (ending in a fn) and roots the fn.
fn entry(vm: &mut ScriptVm, code: &str) -> (ScriptValue, ScriptObjectRef) {
    let f = vm.eval(ScriptMod::new("", "", "loop_heap_steady.splash", 0, 0, code, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    let obj = f.as_object().expect("the script returns its entry fn");
    let root = vm.bx.heap.new_object_ref(obj);
    (f, root)
}

fn object_slots(vm: &ScriptVm) -> u64 {
    vm.heap().heap_stat(HEAP_STAT_OBJECTS_LEN)
}

/// No GC at all: loops free their own scopes and range literals, so the object
/// table never grows. 9000 calls x 1000 outer iterations: before, each outer
/// iteration left two objects (the inner range and the pinned outer iteration
/// scope), 18M in all, past the table's 2^24 slots.
#[test]
fn loops_free_their_scopes_and_ranges_without_gc() {
    let mut vm = test_vm();
    let (f, _root) = entry(
        &mut vm,
        "let f = fn() { let c = 0 for i in 0..1000 { for j in 0..2 { c += j } } let k = 0 while k < 3 { for q in 0..2 { k += 1 } } return c + k }\nf",
    );
    let mut warm = 0;
    for n in 0..9000 {
        let v = vm.call(f, &[]);
        assert_eq!(v.as_number(), Some(1004.0));
        if n == 10 {
            warm = object_slots(&vm);
        }
    }
    assert_eq!(object_slots(&vm), warm, "the object table grew without GC: loops leaked objects");
}

/// The rwbench splash_eval workload under the host GC cadence: the live heap
/// is the same after 300 calls as after 20, and the table stays small.
#[test]
fn repeated_eval_reaches_a_steady_heap() {
    let mut vm = test_vm();
    let (f, _root) = entry(&mut vm, BENCH);
    let mut live_warm = 0;
    let mut slots_warm = 0;
    for n in 0..300 {
        let v = vm.call(f, &[]);
        assert!(v.as_number().is_some());
        if vm.heap().needs_gc() {
            vm.gc();
        }
        if n == 20 {
            vm.gc();
            live_warm = vm.heap().gc_live_len();
            slots_warm = object_slots(&vm);
        }
    }
    vm.gc();
    assert_eq!(vm.heap().gc_live_len(), live_warm, "live heap grew across calls");
    assert!(object_slots(&vm) <= slots_warm.max(4096), "object table grew: {} slots", object_slots(&vm));
}

/// Small pod churn shares the live objects' GC headroom; payload categories
/// still trigger independently at their own doubling thresholds.
#[test]
fn objects_and_pods_share_gc_growth() {
    let mut vm = test_vm();
    let (pods, _pods_root) = entry(
        &mut vm,
        "use mod.pod.*
let objects = []
for i in 0..2048 { objects.push({value: i}) }
fn(n) { for i in 0..n { let p = vec3f(1, 2, 3) } return objects[0].value }",
    );
    let (handles, _handles_root) = entry(
        &mut vm,
        "fn(n) { for i in 0..n { let doc = \"<p/>\".parse_html() } return 0 }",
    );
    vm.call(pods, &[1usize.into()]);
    vm.call(handles, &[1usize.into()]);
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "setup: pod and handle constructors execute without errors: {errors:?}");
    vm.gc();
    let baseline = vm.heap().gc_live_counts();
    let combined = baseline.objects + baseline.pods;
    assert!(baseline.objects >= 2048, "a: the large live object set is rooted");

    let value = vm.call(pods, &[(combined - 1).into()]);
    assert_eq!(value.as_number(), Some(0.0), "a: rooted objects survive the baseline GC");
    let grown = vm.heap().gc_live_counts();
    assert!(grown.pods >= 128 && grown.pods > baseline.pods * 2, "a: pods alone exceed their minimum and double");
    assert_eq!(grown.objects + grown.pods, combined * 2 - 1, "a: combined growth is just below doubling");
    assert!(!vm.heap().needs_gc(), "a: pod churn below combined doubling does not trigger GC");

    vm.call(pods, &[1usize.into()]);
    let grown = vm.heap().gc_live_counts();
    assert_eq!(grown.objects + grown.pods, combined * 2, "b: objects and pods have doubled together");
    assert!(vm.heap().needs_gc(), "b: combined doubling triggers GC");

    vm.gc();
    assert_eq!(vm.heap().gc_live_counts().pods, baseline.pods, "c: short-lived pods return to their baseline after GC");
    assert!(!vm.heap().needs_gc(), "c: collection resets the growth baseline");

    vm.gc();
    let baseline = vm.heap().gc_live_counts();
    let strings_to_double = (baseline.strings * 2).max(256) - baseline.strings;
    for i in 0..strings_to_double - 1 {
        vm.bx.heap.new_string_from_str(&format!("gc growth temporary string {i}"));
    }
    assert!(!vm.heap().needs_gc(), "d strings: below their own threshold does not trigger GC");
    vm.bx.heap.new_string_from_str("gc growth final temporary string");
    let grown = vm.heap().gc_live_counts();
    assert_eq!(grown.strings, (baseline.strings * 2).max(256), "d strings: reach their own doubling threshold and minimum");
    assert_eq!(grown.objects + grown.pods, baseline.objects + baseline.pods, "d strings: objects and pods did not grow");
    assert!(vm.heap().needs_gc(), "d strings: strings alone trigger GC");

    vm.gc();
    let baseline = vm.heap().gc_live_counts();
    let arrays_to_double = (baseline.arrays * 2).max(128) - baseline.arrays;
    for _ in 0..arrays_to_double - 1 {
        vm.bx.heap.new_array();
    }
    assert!(!vm.heap().needs_gc(), "d arrays: below their own threshold does not trigger GC");
    vm.bx.heap.new_array();
    let grown = vm.heap().gc_live_counts();
    assert_eq!(grown.arrays, (baseline.arrays * 2).max(128), "d arrays: reach their own doubling threshold and minimum");
    assert_eq!(grown.objects + grown.pods, baseline.objects + baseline.pods, "d arrays: objects and pods did not grow");
    assert!(vm.heap().needs_gc(), "d arrays: arrays alone trigger GC");

    vm.gc();
    let baseline = vm.heap().gc_live_counts();
    let handles_to_double = (baseline.handles * 2).max(64) - baseline.handles;
    vm.call(handles, &[(handles_to_double - 1).into()]);
    assert!(!vm.heap().needs_gc(), "d handles: below their own threshold does not trigger GC");
    vm.call(handles, &[1usize.into()]);
    let grown = vm.heap().gc_live_counts();
    assert_eq!(grown.handles, (baseline.handles * 2).max(64), "d handles: reach their own doubling threshold and minimum");
    assert_eq!(grown.objects + grown.pods, baseline.objects + baseline.pods, "d handles: objects and pods did not grow");
    assert!(vm.heap().needs_gc(), "d handles: handles alone trigger GC");
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "a-d: script loops execute without errors: {errors:?}");
}

/// What escapes a loop keeps what it reads: closures made in nested loop bodies
/// (reading the loop variables, a body `let` and the call scope) and a loop
/// scope handed out with `scope` stay valid after the call returns, across GC.
#[test]
fn escaped_loop_scopes_keep_their_parents() {
    let mut vm = test_vm();
    let (f, _root) = entry(
        &mut vm,
        "let mk = fn() { let fs = [] let base = 10 for i in 0..3 { for j in 0..2 { let k = i * 2 fs.push(fn() { return base + k + i + j }) } } return fs }
let sc = fn() { for i in 0..2 { let q = 5 + i return scope } }
let run = fn() { let fs = mk() let s = sc() let t = 0 for f in fs { t += f() } return t + s.q + s.i }
run",
    );
    for _ in 0..5 {
        assert_eq!(vm.call(f, &[]).as_number(), Some(86.0));
        vm.gc();
    }
    assert!(vm.take_errors().is_empty());
}

/// A range that is not a literal `for` source is not freed by the loop: the
/// same range object iterates twice through a fn argument and is read after.
#[test]
fn a_shared_range_survives_its_loops() {
    let mut vm = test_vm();
    let (f, _root) = entry(
        &mut vm,
        "let walk = fn(r) { let t = 0 for i in r { t += i } for i in r { t += i } return t + r.end }
let pick = fn(c) { let r = 2..4 for i in if c { r } else { 0..1 } { } return r.start }
fn() { return walk(0..4) + pick(true) }",
    );
    for _ in 0..3 {
        assert_eq!(vm.call(f, &[]).as_number(), Some(18.0));
        vm.gc();
    }
    assert!(vm.take_errors().is_empty());
}

const BENCH: &str = r#"
let fib = fn(n) {
    if n < 2 { return n }
    return fib(n - 1) + fib(n - 2)
}
let sieve = fn(n) {
    let flags = []
    for i in 0..n { flags.push(1) }
    let count = 0
    for i in 2..n {
        if flags[i] == 1 {
            count += 1
            let j = i * 2
            for k in 0..n {
                if j >= n { break }
                flags[j] = 0
                j += i
            }
        }
    }
    return count
}
let bodies = fn(steps) {
    let ps = []
    for i in 0..5 { ps.push({x: i * 1.5, y: i * 0.5 - 1.0, vx: 0.0, vy: 0.1 * i}) }
    for s in 0..steps {
        for i in 0..5 {
            let a = ps[i]
            for j in 0..5 {
                if i != j {
                    let b = ps[j]
                    let dx = b.x - a.x
                    let dy = b.y - a.y
                    let d2 = dx * dx + dy * dy + 0.01
                    a.vx += dx / d2 * 0.001
                    a.vy += dy / d2 * 0.001
                }
            }
        }
        for i in 0..5 {
            let a = ps[i]
            a.x += a.vx
            a.y += a.vy
        }
    }
    let e = 0.0
    for i in 0..5 { e += ps[i].x * 3.0 + ps[i].y }
    return e
}
fn() {
    return fib(12) * 1000000.0 + sieve(500) * 1000.0 + bodies(10)
}
"#;
