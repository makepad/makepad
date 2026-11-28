use makepad_script::*;
use makepad_script::value_map::ValueMap;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ptr;

struct Failing;

// Arm inside the script, after parsing. Once the first grow fails, even a
// one-byte diagnostic or a free-list allocation during unwind is refused.
const GROW: usize = 4096;
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static REFUSED: Cell<usize> = const { Cell::new(0) };
    static REFUSE_FROM: Cell<Option<usize>> = const { Cell::new(None) };
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn refuse(size: usize) -> bool {
    if let Some(count) = ALLOCATIONS.get() {
        ALLOCATIONS.set(Some(count + 1));
        if REFUSE_FROM.get().is_some_and(|first| count >= first) { return true; }
    }
    ARMED.try_with(|armed| armed.get()).unwrap_or(false) && REFUSED.with(|count| {
        if count.get() != 0 || size >= GROW {
            count.set(count.get() + 1);
            true
        } else {
            false
        }
    })
}

// SAFETY: successful requests and all frees are forwarded with their
// original layout. A refused reallocation leaves its original block intact.
unsafe impl GlobalAlloc for Failing {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) { ptr::null_mut() } else { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refuse(layout.size()) { ptr::null_mut() } else { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refuse(size) { ptr::null_mut() } else { System.realloc(p, layout, size) }
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) { System.dealloc(p, layout) }
}

#[global_allocator]
static ALLOCATOR: Failing = Failing;

// Return the number of allocator requests, including refused ones. Disarm on unwind too.
fn requests_during(refuse_from: Option<usize>, action: impl FnOnce()) -> usize {
    struct Disarm;
    impl Drop for Disarm {
        fn drop(&mut self) { REFUSE_FROM.set(None); ALLOCATIONS.set(None); }
    }
    let _guard = Disarm;
    ALLOCATIONS.set(Some(0));
    REFUSE_FROM.set(refuse_from);
    action();
    ALLOCATIONS.get().unwrap()
}

fn live_counts(vm: &ScriptVm) -> [usize; 7] {
    let c = vm.heap().gc_live_counts();
    [c.objects, c.arrays, c.strings, c.pods, c.handles, c.regexes, vm.heap().heap_stat(HEAP_STAT_POD_TYPES) as usize]
}

fn map_with_holes(keys: u64, removed: u64) -> ValueMap<u64, u64> {
    let mut map = ValueMap::default();
    for key in 0..keys { map.insert(key, key); }
    for key in 0..removed { map.remove(&key); }
    map
}

#[test]
fn value_map_first_spill_refuses_index_box_allocation() {
    let trial = |fail| {
        let mut map = map_with_holes(16, 0);
        let mut result = Ok(());
        let count = requests_during(fail, || result = map.try_reserve_insert());
        if let Some(k) = fail {
            assert!(result.is_err(), "first spill must propagate refusal {k}");
            assert_eq!(count, k + 1, "first spill must stop at refusal {k}");
            assert_eq!(map.len(), 16, "a refused spill must preserve the length");
            assert!((0..16).all(|k| map.get(&k) == Some(&k)), "a refused spill must preserve every key");
        } else {
            result.expect("the first spill should reserve successfully");
            let inserts = requests_during(Some(0), || { map.insert(16, 16); });
            assert_eq!(inserts, 0, "inserting the reserved seventeenth key must allocate nothing");
            assert_eq!(map.get(&16), Some(&16), "the new index must find the seventeenth key");
        }
        count
    };
    let requests = trial(None);
    assert!(requests >= 3, "the first spill must exercise entry, bucket and index allocations");
    for k in 0..requests { trial(Some(k)); }
}

fn check_bulk_reserve(keys: u64, removed: u64, additional: usize, cache_ranks: bool) {
    let setup = || {
        let map = map_with_holes(keys, removed);
        if cache_ranks {
            assert_eq!(map.get_index(0), Some(&(removed, removed)), "prime ranks on each fresh map");
        }
        map
    };
    let end = 1000 + additional as u64;
    let insert_batch = |map: &mut ValueMap<u64, u64>| {
        for k in 1000..end { map.insert(k, k); }
    };
    let mut map = setup();
    let requests = requests_during(None, || map.try_reserve(additional).expect("reserve the batch"));
    let inserts = requests_during(None, || insert_batch(&mut map));
    assert_eq!(inserts, 0, "inserting the reserved batch must allocate nothing");
    assert!((removed..keys).chain(1000..end).all(|k| map.get(&k) == Some(&k)), "all surviving and new keys must be indexed");
    assert!((0..removed).all(|k| !map.contains_key(&k)), "removed keys must stay absent");
    assert_eq!(map.get_index(map.len() - 1), Some(&(end - 1, end - 1)), "the last rank must find the last insertion");

    assert!(requests >= 2, "the batch must exercise both entries and index storage");
    for k in 0..requests {
        let mut map = setup();
        let mut result = Ok(());
        let count = requests_during(Some(k), || result = map.try_reserve(additional));
        assert!(result.is_err(), "bulk reserve must propagate refusal {k}");
        assert_eq!(count, k + 1, "bulk reserve must stop at refusal {k}");
        assert_eq!(map.len(), (keys - removed) as usize, "a refused reserve must preserve the length");
        assert!((removed..keys).all(|k| map.get(&k) == Some(&k)), "a refused reserve must preserve lookups");
        assert_eq!(map.get_index(0), Some(&(removed, removed)), "a refused reserve must preserve the first rank");
    }
    let mut map = setup();
    map.try_reserve(additional).expect("reserve before denying every allocation");
    assert_eq!(requests_during(Some(0), || insert_batch(&mut map)), 0, "the reserved batch must work with allocation disabled");
}

#[test]
fn value_map_bulk_reserve_compacts_holes_without_allocating_on_insert() {
    check_bulk_reserve(64, 48, 100, false);
}

#[test]
fn value_map_bulk_reserve_covers_rank_cache_without_allocating_on_insert() {
    check_bulk_reserve(32, 8, 8, true);
}

fn host_vm(host: &mut ScriptVmHost) -> ScriptVm<'_> {
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm
}

fn visit_probe(vm: &mut ScriptVm, args: &[(LiveId, ScriptValue)]) -> (ScriptValue, std::rc::Rc<Cell<usize>>) {
    let calls = std::rc::Rc::new(Cell::new(0));
    let count = calls.clone();
    let module = vm.new_module(id!(probe));
    vm.add_method(module, id_lut!(visit), args, move |_, _| {
        count.set(count.get() + 1);
        42.into()
    });
    (vm.value(module, id!(visit)), calls)
}

fn drain_free_objects(vm: &mut ScriptVm) {
    let free = vm.heap().objects_len() - vm.heap().gc_live_counts().objects;
    for _ in 0..free { vm.new_object(); }
}

fn warm_call_scope(vm: &mut ScriptVm, function: ScriptValue) {
    drain_free_objects(vm);
    let scope = vm.heap_mut().new_with_proto(function);
    vm.release_transient(scope.into());
}

fn assert_host_refusal(vm: &mut ScriptVm, result: ScriptValue, reason: &str) {
    let refusal = vm.heap_mut().take_memory_refusal()
        .unwrap_or_else(|| panic!("the call must deliver its physical refusal (returned {result:?})"));
    assert_eq!(refusal.reason, reason, "the host must receive the original reservation failure");
    assert!(refusal.ip.is_unknown(), "a host call must report a host instruction pointer");
    assert_eq!(result, ScriptValue::script_err_limit(refusal.ip), "the return value must report the refusal");
    assert!(vm.heap_mut().take_memory_refusal().is_none(), "the host must receive the refusal only once");
    assert!(vm.take_errors().is_empty(), "physical refusal must produce no formatted diagnostic");
}

const ADD_PROPERTY: &str = "out of memory while adding an object property";

#[test]
fn physical_oom_after_logical_refusal_leaves_no_stale_limit_error() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let arr = vm.heap_mut().new_array();
    let obj = vm.heap_mut().new_object();
    let prop_key = vm.heap_mut().new_string_from_str("scoped_oom_supersedes_budget_prop");
    let input = std::rc::Rc::new(Cell::new(None));
    let requests = std::rc::Rc::new(Cell::new(0));
    let module = vm.new_module(id!(probe));
    let text = input.clone();
    let count = requests.clone();
    vm.add_method(module, id_lut!(push), script_args_def!(), move |vm, _| {
        if let Some(text) = text.get() {
            vm.heap_mut().new_string_from_str(text);
            count.set(requests_during(Some(0), || {
                if text == "too_long" {
                    vm.heap_mut().array_push(arr, 1.into(), NoTrap);
                } else {
                    vm.heap_mut().set_value(obj, prop_key, 1.into(), NoTrap);
                }
            }));
        }
        NIL
    });
    let probe = vm.value(module, id!(push));
    let outcomes = [false, true].map(|scoped| {
        input.set(None);
        assert_eq!(vm.call(probe, &[]), NIL, "warm the zero-argument native scope");
        vm.capture_errors();
        vm.heap_mut().set_max_string_bytes((!scoped).then_some(4));
        requests.set(usize::MAX);
        input.set(Some(if scoped { "a_heap_string" } else { "too_long" }));
        let (value, pending_after_call, exceeded) = if scoped {
            let ((value, pending), report) = vm.with_heap_allocation_limit(0, |vm| {
                let value = vm.call(probe, &[]);
                (value, vm.heap().allocation_error_pending())
            });
            (value, pending, Some(report.exceeded))
        } else {
            (vm.call(probe, &[]), vm.heap().allocation_error_pending(), None)
        };
        let pending = vm.heap().allocation_error_pending();
        let refusal = vm.heap_mut().take_memory_refusal();
        let errors = vm.take_errors();
        let recovered = vm.eval(ScriptMod::new("", "", "recovery", 0, 0, "6 * 7", &[])).as_number();
        let exceeded = exceeded.unwrap_or_else(|| vm.heap_mut().take_string_limit_exceeded());
        vm.heap_mut().set_max_string_bytes(None);
        let reason = if scoped { "out of memory while naming a property" } else { "out of memory while pushing an array element" };
        let refusal = refusal.expect("the host must receive the physical refusal");
        assert_eq!(requests.get(), 1, "the native must reach exactly one refused allocator request");
        assert_eq!(refusal.reason, reason, "the native must report the targeted physical refusal");
        assert_eq!(value, ScriptValue::script_err_limit(refusal.ip), "the call must return the physical refusal");
        assert!(exceeded, "the host must still see the exceeded logical limit");
        assert_eq!(vm.heap().map_len(obj), 0, "refused naming must leave the plain object empty");
        (pending_after_call, pending, errors, recovered)
    });
    let pending = outcomes.each_ref().map(|case| case.0);
    assert_eq!(pending, [false, false], "outermost entry exit must clear both superseded logical errors: {outcomes:?}");
    for (_, pending, errors, recovered) in outcomes {
        assert!(errors.is_empty(), "the superseded logical error must produce no diagnostic: {errors:?}");
        assert!(!pending, "no allocation error may remain after the budget scope closes");
        assert_eq!(recovered, Some(42.0), "the next evaluation must start cleanly");
    }
}

#[test]
fn host_call_physical_oom_skips_native_and_reports_once() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let (probe, calls) = visit_probe(&mut vm, script_args_def!(a = NIL));
    warm_call_scope(&mut vm, probe);
    let mut result = NIL;
    let count = requests_during(Some(0), || result = vm.call(probe, &[1.into()]));
    assert_eq!(count, 1, "the warm call must stop at the refused argument binding");
    assert_eq!(calls.get(), 0, "refused argument binding must skip the native");
    assert_host_refusal(&mut vm, result, ADD_PROPERTY);

    vm.capture_errors();
    vm.gc_and_compact();
    drain_free_objects(&mut vm);
    let count = requests_during(Some(0), || result = vm.call(probe, &[1.into()]));
    assert_eq!(count, 1, "the cold call must stop at the refused scope allocation");
    assert_eq!(calls.get(), 0, "refused scope allocation must skip the native");
    assert_host_refusal(&mut vm, result, "out of memory while creating an object");

    vm.capture_errors();
    let big = vm.heap_mut().new_string_from_str(&"a".repeat(64));
    let count = requests_during(Some(0), || { vm.heap_mut().new_string_concat(big, big); });
    assert_eq!(count, 1, "the raw concat must reach its refused allocation");
    assert_eq!(vm.call(probe, &[1.into()]), 42.into(), "a new call must recover despite an untaken raw refusal");
    assert_eq!(calls.get(), 1, "only the recovery call may run the native");
    assert!(vm.heap_mut().take_memory_refusal().is_none(), "the new entry must clear the raw refusal");
    assert!(vm.take_errors().is_empty(), "recovery must emit no diagnostic");
}

#[test]
fn host_call_script_physical_oom_never_runs_with_default_arg() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let f = vm.eval(ScriptMod::new("", "", "default_arg", 0, 0, "fn(a = 99) { a }", &[]));
    warm_call_scope(&mut vm, f);
    let mut result = NIL;
    let count = requests_during(Some(0), || result = vm.call(f, &[1.into()]));
    assert_eq!(count, 1, "the script call must stop at the refused argument binding");
    assert_host_refusal(&mut vm, result, ADD_PROPERTY);
    assert_eq!(vm.call(f, &[1.into()]), 1.into(), "the next call must receive its supplied argument");
}

#[test]
fn host_call_with_args_object_physical_oom_skips_native() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let (probe, calls) = visit_probe(&mut vm, script_args_def!(x = NIL));
    let args = vm.heap_mut().new_object();
    vm.heap_mut().vec_push(args, NIL, 1.into(), NoTrap);
    warm_call_scope(&mut vm, probe);
    let mut result = NIL;
    let count = requests_during(Some(0), || result = vm.call_with_args_object(probe, args));
    assert_eq!(count, 1, "positional copying must stop at its refused binding");
    assert_eq!(calls.get(), 0, "a refused positional copy must skip the native");
    assert_host_refusal(&mut vm, result, ADD_PROPERTY);

    vm.heap_mut().set_value_def(args, id!(self).into(), 123.into());
    warm_call_scope(&mut vm, probe);
    let count = requests_during(None, || result = vm.call_with_args_object(probe, args));
    assert_eq!(result, 42.into(), "a small receiver map must dispatch successfully");
    assert_eq!(count, 1, "a small receiver map must allocate only for argument binding");
    calls.set(0);
    for key in [id!(a), id!(b), id!(c), id!(d), id!(e)] {
        vm.heap_mut().set_value_def(args, key.into(), 1.into());
    }
    warm_call_scope(&mut vm, probe);
    let requests = requests_during(None, || result = vm.call_with_args_object(probe, args));
    assert_eq!(result, 42.into(), "a large receiver map must dispatch successfully");
    assert_eq!(requests, 3, "the large case must grow the binding, temporary copy, and destination map");
    calls.set(0);
    for k in 0..requests {
        vm.capture_errors();
        warm_call_scope(&mut vm, probe);
        let count = requests_during(Some(k), || result = vm.call_with_args_object(probe, args));
        assert_eq!(count, k + 1, "copying arguments must stop at refusal {k}");
        assert_eq!(calls.get(), 0, "the native must never receive a partial map at refusal {k}");
        assert_host_refusal(&mut vm, result, ADD_PROPERTY);
    }
    assert_eq!(vm.call_with_args_object(probe, args), 42.into(), "a complete argument copy must recover");
    assert_eq!(calls.get(), 1, "only the recovery call may run after the refusal sweep");
}

#[test]
fn host_call_entry_physical_oom_poisons_policy_and_skips_native() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let (probe, calls) = visit_probe(&mut vm, script_args_def!(a = NIL));
    drain_free_objects(&mut vm);
    vm.bx.set_policy(ScriptVmPolicy::untrusted()).expect("install the entry policy");
    assert_eq!(vm.call_entry(&mut ScriptCallReq::new(CALL_ARGS, probe)), Ok(42.into()), "warm the policy entry and call scope");
    calls.set(0);
    let args = [1.into()];
    let mut req = ScriptCallReq::new(CALL_ARGS, probe);
    req.args = &args;
    let mut outcome = Ok(NIL);
    let count = requests_during(Some(0), || outcome = vm.call_entry(&mut req).map_err(Clone::clone));
    assert_eq!(count, 1, "the warm policy entry must stop at the refused argument binding");
    assert_eq!(calls.get(), 0, "a policy entry must skip the native on physical refusal");
    assert_eq!(outcome, Err(Poison::Limit(Limit::Allocation)), "the structured outcome must identify physical exhaustion");
    assert_eq!(vm.bx.poisoned(), Some(&Poison::Limit(Limit::Allocation)), "the allocation poison must persist");
    let refusal = vm.heap_mut().take_memory_refusal().expect("the policy host must receive the physical refusal");
    assert_eq!(refusal.reason, ADD_PROPERTY, "the policy entry must refuse the binding");
    assert!(vm.heap_mut().take_memory_refusal().is_none(), "the policy entry must deliver its refusal once");
    assert!(vm.take_errors().is_empty(), "policy refusal must emit no formatted diagnostic");
}

#[test]
fn host_apply_transform_physical_oom_returns_limit_error() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = host_vm(&mut host);
    let arr = vm.heap_mut().new_array();
    let obj = vm.heap_mut().new_object();
    let transform = vm.add_apply_transform_fn(move |vm, _| {
        vm.heap_mut().array_push(arr, 1.into(), NoTrap);
        NIL
    });
    vm.heap_mut().set_object_apply_transform(obj, transform);
    let mut out = None;
    let count = requests_during(Some(0), || out = vm.call_apply_transform(obj.into()));
    assert_eq!(count, 1, "the transform must stop at its refused array push");
    assert_host_refusal(&mut vm, out.expect("the transform must be found"), "out of memory while pushing an array element");
    assert_eq!(vm.heap().array_len(arr), 0, "a refused push must preserve the array");
    assert_eq!(vm.call_apply_transform(obj.into()), Some(NIL), "the next transform must return normally");
    assert_eq!(vm.heap().array_len(arr), 1, "only the recovery transform may append");
}

#[test]
fn bind_map_entry_untracked_oom_refuses_cleanly() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let proto = vm.heap_mut().new_object();
    let key = id!(param).into();
    vm.heap_mut().vec_push(proto, key, NIL, NoTrap);
    let args = vm.heap_mut().new_with_proto(proto.into());
    let count = requests_during(Some(0), || {
        vm.heap_mut().unnamed_fn_arg(args, 1.into(), NoTrap);
    });
    assert_eq!(count, 1, "a refused argument binding must issue only its reservation");
    let refusal = vm.heap_mut().take_memory_refusal().expect("binding must report the physical refusal");
    assert_eq!(refusal.reason, "out of memory while adding an object property", "the refused binding must identify its map growth");
    assert_eq!(vm.heap().map_len(args), 0, "a refused binding must leave the argument map empty");
    assert!(vm.take_errors().is_empty(), "a physical refusal must queue no secondary error");
    assert_eq!(vm.eval(ScriptMod::new("", "", "recovery", 1, 0, "6 * 7", &[])).as_number(),
        Some(42.0), "the VM must recover after the binding refusal");
}

#[test]
fn merge_and_extend_object_oom_refuses_cleanly() {
    for (nested, copy_map, reserves) in [(false, true, 3), (true, true, 2), (true, false, 1)] {
        for refused in 0..reserves {
            let mut host = ScriptVmHost::new((), ());
            let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
            vm.capture_errors();
            let src = vm.heap_mut().new_object();
            vm.heap_mut().vec_push(src, NIL, 1.into(), NoTrap);
            vm.heap_mut().set_value_def(src, id!(x).into(), 2.into());
            let outer = vm.heap_mut().new_object();
            vm.heap_mut().vec_push(outer, NIL, src.into(), NoTrap);
            let dst = vm.heap_mut().new_object();
            let count = requests_during(Some(refused), || {
                if nested {
                    vm.heap_mut().vec_push_vec_of_vec(dst, outer, copy_map, NoTrap);
                } else {
                    vm.heap_mut().merge_object(dst, src, NoTrap);
                }
            });
            assert_eq!(count, refused + 1, "container copying must stop at its first refused reservation");
            assert!(vm.heap_mut().take_memory_refusal().is_some(), "the host must receive the physical refusal");
            assert_eq!(vm.heap().vec_len(dst), 0, "a refused copy must preserve the destination vector");
            assert_eq!(vm.heap().map_len(dst), 0, "a refused copy must preserve the destination map");
            assert!(vm.take_errors().is_empty(), "a refused copy must queue no secondary error");
            assert_eq!(vm.eval(ScriptMod::new("", "", "recovery", 1, 0, "6 * 7", &[])).as_number(),
                Some(42.0), "the VM must recover after the copy refusal");
        }
    }
}

#[test]
fn collection_and_static_walk_need_no_allocation() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let make = vm.eval(ScriptMod::new("", "", "make_graph", 0, 0, &r#"use mod.pod.*
        fn(n) {
            let P = struct{v: f32}
            let proto = {weight: 1}
            let shared = {weight: 2}
            let common = [3]
            let wide = []
            let keys = {}
            let head = nil
            for i in 0..n {
                head = proto{next: head, ring: head, rank: i, shared: shared, common: common}
                wide.push(head)
                keys[i] = head
            }
            wide[0].ring = head
            let root = {head: head, wide: wide, keys: keys, pod: P(4f)}
            root.again = root
            root.back = [root]
            return root
        }"#, &[]));
    let walk = vm.eval(ScriptMod::new("", "", "walk_graph", 0, 0, &r#"fn(root, n) {
        let sum = 0
        let p = root.head
        let r = root.head
        for i in 0..n {
            sum += p.weight + root.wide[i].shared.weight + root.keys[i].common[0]
            sum += p.rank + root.wide[i].rank + root.keys[i].rank + r.rank
            p = p.next
            r = r.ring
        }
        if p != nil { return -1 }
        return sum + r.weight + root.again.back[0].pod.v
    }"#, &[]));
    let _make = vm.heap_mut().new_fn_ref(make.as_object().expect("the graph builder compiled"));
    let _walk = vm.heap_mut().new_fn_ref(walk.as_object().expect("the graph walk compiled"));
    let n = 1024usize;
    let root = vm.call(make, &[n.into()]).as_object().expect("the graph builder returned its root");
    let keep = vm.heap_mut().new_object_ref(root);
    vm.gc();
    let expected = live_counts(&vm);
    let dead = vm.call(make, &[n.into()]).as_object().expect("the second graph was built");
    assert!(vm.heap().object_alive(dead), "the unreachable graph exists before collection");
    // The string buffer cache has finite spare capacity even when the table can free every slot.
    for i in 0..n { vm.heap_mut().new_string_from_str(&format!("unreachable string {i}")); }

    let requests = requests_during(Some(0), || vm.gc());
    assert_eq!(requests, 0, "collecting the reachable and unreachable graphs must ask for no memory");
    assert!(!vm.heap().object_alive(dead), "the unreachable cyclic graph was freed");
    assert_eq!(live_counts(&vm), expected, "only the reachable half survives, including its pod type");
    vm.gc();
    assert_eq!(live_counts(&vm), expected, "the next collection frees nothing");
    let verify = |vm: &mut ScriptVm| {
        let value = vm.call(walk, &[root.into(), n.into()]);
        assert!(vm.take_errors().is_empty(), "all edges in the script walk must remain readable");
        assert_eq!(value.as_number(), Some((6 * n + 4 * n * (n - 1) / 2 + 5) as f64), "the script reached each shape through its surviving edges");
    };
    verify(&mut vm);
    let requests = requests_during(Some(0), || vm.heap_mut().set_static(root.into()));
    assert_eq!(requests, 0, "making the shared cyclic graph static must ask for no memory");
    drop(keep);
    vm.gc();
    verify(&mut vm);
    vm.gc_and_compact();
    let garbage = vm.heap_mut().new_object();
    let requests = requests_during(Some(0), || vm.gc());
    assert_eq!(requests, 0, "collection in reused slots after compaction must ask for no memory");
    assert!(!vm.heap().object_alive(garbage), "the post-compaction collection freed the new garbage");

    // A host may split mark and sweep. The static walk borrows the mark's bitmaps,
    // so reclamation waits for the next full collection.
    let bx = &mut *vm.bx;
    let other = bx.heap.new_object();
    let _other = bx.heap.new_object_ref(other);
    let garbage = bx.heap.new_object();
    let permanent = bx.heap.new_object();
    let child = bx.heap.new_array();
    bx.heap.set_value_def(permanent, id!(child).into(), child.into());
    bx.heap.array_push(child, permanent.into(), NoTrap);
    bx.heap.mark(&bx.threads, &bx.code);
    let requests = requests_during(Some(0), || bx.heap.set_static(permanent.into()));
    assert_eq!(requests, 0, "set_static also needs no allocation between a host's mark and sweep");
    let before_sweep = live_counts(&vm);
    vm.bx.heap.sweep(false);
    assert_eq!(live_counts(&vm), before_sweep, "the abandoned mark's sweep freed nothing");
    assert!(vm.heap().object_alive(garbage), "the skipped sweep left the garbage in place");
    assert!(vm.heap().object_alive(other), "the skipped sweep left the other root alive");

    vm.gc();
    assert!(!vm.heap().object_alive(garbage), "the next full collection freed the garbage");
    assert!(vm.heap().object_alive(other), "the next full collection kept the other root");
    assert!(vm.heap().object_alive(permanent), "the next full collection kept the newly static object");
    assert_eq!(vm.heap().value(permanent, id!(child).into(), NoTrap), child.into(), "the static object kept its array");
    assert_eq!(vm.heap().array_index(child, 0, NoTrap), permanent.into(), "the static array kept its edge back to the object");
}

#[test]
fn a_refused_gc_preflight_leaves_the_graph_and_bodies_intact() {
    // Each trial needs fresh body scratch so the same k selects the same reserve.
    fn trial(fail: Option<usize>) -> (usize, [usize; 7]) {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        let f = vm.eval_transient(ScriptMod::new("", "", "temporary", 0, 0, &"fn() { return 7 }", &[]));
        let Some(ScriptFnPtr::Script(ip)) = vm.heap().as_fn(f.as_object().unwrap()) else { panic!("expected script code") };
        let read = vm.eval(ScriptMod::new("", "", "read", 0, 0, &"fn(o) { return o.loopback[0].value }", &[]));
        let _read = vm.heap_mut().new_fn_ref(read.as_object().unwrap());
        let ring = |heap: &mut ScriptHeap| {
            let o = heap.new_object();
            let a = heap.new_array();
            heap.array_push(a, o.into(), NoTrap);
            heap.set_value_def(o, id!(loopback).into(), a.into());
            heap.set_value_def(o, id!(value).into(), 42.into());
            o
        };
        let root = ring(vm.heap_mut());
        let _keep = vm.heap_mut().new_object_ref(root);
        let garbage = ring(vm.heap_mut());
        let before = live_counts(&vm);
        let count = requests_during(fail, || vm.gc());
        if fail.is_some_and(|k| k < count) {
            // Outside an interpreter the refusal is reported to stderr at once. That report must
            // not allocate either: on macOS the first lock of std::io::stderr() boxes its mutex.
            assert_eq!(count, fail.unwrap() + 1, "a refused preflight, or its report, made another request");
            let refusal = vm.heap_mut().take_memory_refusal().expect("a refused collection records its reason");
            assert_eq!(refusal.reason, "out of memory while collecting garbage", "the refusal names collection");
            assert!(vm.heap_mut().take_memory_refusal().is_none(), "one refused collection reports once");
            assert_eq!(live_counts(&vm), before, "a refused preflight changed the live counts");
            assert!(vm.heap().object_alive(garbage), "a refused collection swept a slot");
            assert!(!matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free),
                "a refused collection freed a transient body");
            vm.gc();
        }
        assert!(vm.heap_mut().take_memory_refusal().is_none(), "a completed collection reported a refusal");
        assert!(!vm.heap().object_alive(garbage), "a completed collection freed the dead cycle");
        assert!(matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free),
            "a completed collection reclaimed the dead body");
        let after = live_counts(&vm);
        assert_eq!(vm.call(read, &[root.into()]).as_number(), Some(42.0), "the rooted cycle still walks after collection");
        (count, after)
    }
    let (n, expected) = trial(None);
    assert!(n > 0, "cold body scratch exercises at least one fallible reserve");
    for k in 0..=n {
        let (count, after) = trial(Some(k));
        assert_eq!(count, (k + 1).min(n), "the same preflight reserve was reached at request {k}");
        assert_eq!(after, expected, "after refusal at request {k}, retry kept exactly the live graph");
    }
}

#[test]
fn reclaiming_a_transient_body_needs_no_allocation() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let idle = || ScriptMod::new("", "", "idle", 0, 0, &"nil", &[]);
    vm.eval(idle());
    let f = vm.eval_transient(ScriptMod::new("", "", "transient", 0, 0, &"fn() { return 7 }", &[]));
    let kept = vm.heap_mut().new_fn_ref(f.as_object().expect("the transient body returns a function"));
    let Some(ScriptFnPtr::Script(ip)) = vm.heap().as_fn(kept.as_object()) else { panic!("the function has script code"); };
    vm.eval(idle());
    vm.gc();
    assert!(!matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free),
        "the rooted function keeps its body during the warm-up collection");
    drop(kept);
    let requests = requests_during(Some(0), || vm.gc());
    assert!(matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free),
        "the measured collection reclaimed the transient body");
    assert_eq!(requests, 0, "reclaiming a transient body after preflight warm-up must ask for no memory");
}

#[test]
fn sweeping_dead_regexes_allocates_nothing() {
    const N: usize = 32;
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let code = r#"use mod.std.regex
        fn make(n) {
            let xs = []
            for i in 0..n { xs.push(regex("r" + i, "")) }
            return xs.len()
        }
        make"#;
    let make = vm.eval(ScriptMod::new("", "", "regex_sweep", 0, 0, code, &[]));
    let _root = vm.heap_mut().new_object_ref(make.as_object().expect("the script returns its regex builder"));
    vm.gc();
    let baseline = vm.heap().gc_live_counts().regexes;
    assert_eq!(vm.call(make, &[N.into()]).as_number(), Some(N as f64), "the local array held every regex");
    assert_eq!(vm.heap().gc_live_counts().regexes, baseline + N, "all patterns are distinct");

    let count_gc = |vm: &mut ScriptVm| {
        ALLOCATIONS.set(Some(0));
        vm.gc();
        ALLOCATIONS.replace(None).unwrap()
    };
    let sweep = count_gc(&mut vm);
    assert_eq!(vm.heap().gc_live_counts().regexes, baseline, "the collection freed every local regex");
    vm.gc();
    let empty = count_gc(&mut vm);
    assert_eq!(sweep, empty, "the sweep of 32 dead regexes allocates nothing");
}

#[test]
fn a_refused_grow_unwinds_without_another_allocation() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let probe = vm.new_module(id!(probe));
    vm.add_method(probe, id_lut!(arm), script_args_def!(), |_, _| {
        REFUSED.set(0);
        ARMED.set(true);
        NIL
    });
    vm.add_method(probe, id_lut!(build), script_args_def!(), |vm, _| {
        vm.heap_mut().new_bounded_string_with(|_, buffer| {
            for _ in 0..GROW {
                buffer.append_str("abcdefgh");
                if buffer.is_full() { break; }
            }
        })
    });
    let seed = "x".repeat(GROW / 2);
    // Each level keeps its left operand on the stack: past the thread's first 1024 values.
    let deep = format!("let x = {}1{}", "1 + (".repeat(1100), ")".repeat(1100));
    let sparse = if usize::BITS >= 64 { "xs[1e12] = 1" } else { "xs[4096] = 1" };
    for (case, setup, operation, reason) in [
        ("push", "let xs = []", "loop { xs.push(1) }", "out of memory while extending an array"),
        ("double", "", "loop { text = text + text }", "out of memory while concatenating strings"),
        ("sparse", "let xs = []", sparse, "out of memory while growing a sparse array index"),
        ("argument", "let xs = []", "xs.push(text + text)", "out of memory while concatenating strings"),
        ("frozen", "let o = {text: text}\no.freeze()", "o.text += text", "out of memory while concatenating strings"),
        ("builder", "", "mod.probe.build()", "out of memory while building a bounded string"),
        // Each object keeps the last alive, so the object table itself must grow.
        ("objects", "let head = nil", "loop { head = {next: head} }", "out of memory while creating an object"),
        // A numeric key indexes the vector. Distinct object keys grow just the map.
        ("vector", "let o = {}\nlet k = 0", "loop {\no[k] = k\nk += 1\n}", "out of memory while extending an object vector"),
        ("map", "let o = {}\nlet keys = []\nfor i in 0..512 { keys.push({}) }\nlet k = 0",
            "loop {\no[keys[k]] = k\nk += 1\n}", "out of memory while adding an object property"),
        // Prepare the long text before arming. Its copy into the name table must refuse.
        ("names", "let o = {}\nlet key = text + text + \"property\"",
            "o[key] = 1", "out of memory while naming a property"),
        ("operands", "", deep.as_str(), "out of memory while growing the operand stack"),
        // Freed objects give each call's argument scope a slot, so the frames themselves must grow.
        ("recursion", "let junk = []\nfor i in 0..400 { junk.push({}) }\njunk = nil\nmod.gc.run()",
            "fn f(n) { f(n + 1) }\nf(0)", "out of memory while growing the call stack"),
        // Warm the recursion's thread vectors on a different map. Each live frame
        // then needs another cursor on the same, previously uniterated object.
        ("cursors", "let o = {field: 1}\nfn walk(obj, n) { for k, v in obj { if n > 0 { walk(obj, n - 1) } } }\nwalk({warm: 1}, 160)\nmod.gc.run()",
            "walk(o, 160)", "out of memory while starting a loop"),
    ] {
        let code = format!("fn run() {{\nlet text = \"{seed}\"\n{setup}\nmod.probe.arm()\n{operation}\n}}\ntry run() catch 99");
        let source = ScriptMod::new("", "", &case, 0, 0, &code, &[]);
        vm.capture_errors();
        // The fuel guard also ends a broken push-loop test if its grow never reaches the probe.
        let value = vm.with_instruction_limit(100_000, |vm| vm.eval(source));
        ARMED.set(false);
        assert_eq!(REFUSED.get(), 1, "{case}: only the physical grow requested memory");
        let refusal = vm.heap_mut().take_memory_refusal().expect("the refusal reached the host");
        assert_eq!(refusal.reason, reason, "{case}: the first failed operation was retained");
        assert!(!refusal.ip.is_unknown(), "{case}: the script instruction is known");
        assert_eq!(value, ScriptValue::script_err_limit(refusal.ip), "{case}: the bail escaped catch");
        assert!(vm.heap_mut().take_memory_refusal().is_none(), "{case}: the refusal is delivered once");
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{case}: a secondary diagnostic escaped: {errors:?}");
        assert_eq!(vm.eval(ScriptMod::new("", "", "recovery", 0, 0, &"6 * 7", &[])).as_number(),
            Some(42.0), "{case}: a fresh evaluation on this VM completes");
    }

    for take in [true, false] {
        let text = vm.heap_mut().new_string_from_str(&"h".repeat(GROW * 2));
        vm.capture_errors();
        REFUSED.set(0);
        ARMED.set(true);
        let result = vm.heap_mut().new_string_concat(text, text);
        let refusal = take.then(|| vm.heap_mut().take_memory_refusal());
        ARMED.set(false);
        assert!(result.is_nil(), "host concat returns its failure to its caller");
        assert_eq!(REFUSED.get(), 1, "reporting a host refusal needs no allocation");
        if let Some(refusal) = refusal {
            let refusal = refusal.expect("a host can take its refusal before running any script");
            assert_eq!(refusal.reason, "out of memory while concatenating strings");
            assert!(refusal.ip.is_unknown(), "a host concat has no script instruction");
        }
        assert_eq!(vm.eval(ScriptMod::new("", "", "after_host", 0, 0, &"6 * 7", &[])).as_number(),
            Some(42.0), "an innocent run succeeds even if the host left its refusal untaken");
        assert!(vm.heap_mut().take_memory_refusal().is_none(), "the new run owns no stale refusal");
        assert!(vm.take_errors().is_empty(), "the host failure never became a script diagnostic");
    }

    // The first two stores finish. Fail the third name copy after its key is built.
    let source = format!("let o = {{}}\nfor k in 0..4 {{\nlet name = \"{seed}{seed}loop\" + k\nif k == 2 {{ mod.probe.arm() }}\no[name] = k\n}}");
    vm.capture_errors();
    let value = vm.eval(ScriptMod::new("", "", "naming_loop", 0, 0, &source, &[]));
    ARMED.set(false);
    let refusal = vm.heap_mut().take_memory_refusal().expect("a running naming loop reports its refused store");
    assert_eq!(refusal.reason, "out of memory while naming a property");
    assert_eq!(value, ScriptValue::script_err_limit(refusal.ip), "the naming loop ended at the refused store");
    assert_eq!(REFUSED.get(), 1, "the naming loop unwound without allocating");
    assert!(vm.take_errors().is_empty(), "a refused name queued a diagnostic");
    assert_eq!(vm.eval(ScriptMod::new("", "", "after_naming", 0, 0, &"6 * 7", &[])).as_number(),
        Some(42.0), "a fresh evaluation succeeded on the naming loop's VM and thread");
    assert!(vm.heap_mut().take_memory_refusal().is_none() && vm.take_errors().is_empty(),
        "the new evaluation inherited neither a refusal nor a diagnostic");

    let key = "refusal_test_colliding_property";
    let other = "refusal_test_first_spelling";
    let id = LiveId::from_str(key);
    assert!(LiveId::lut_insert(id, other, true));
    let store = vm.eval(ScriptMod::new("", "", "colliding_store", 0, 0, &"fn(o, k) { o[k] = 9 }", &[]));
    let object = vm.heap_mut().new_object();
    vm.heap_mut().set_value_def(object, id.into(), 7.into());
    let text = vm.heap_mut().new_string_from_str(key);
    vm.capture_errors();
    vm.call(store, &[object.into(), text]);
    let errors = vm.take_errors();
    assert_eq!(errors.len(), 1, "a collision reported once: {errors:?}");
    assert!(errors[0].contains(&format!("property name `{key}` collides with `{other}`")),
        "the diagnostic distinguishes both spellings: {errors:?}");
    assert_eq!(vm.heap().value(object, id.into(), NoTrap).as_number(), Some(7.0),
        "a colliding key cannot overwrite the existing property");
    assert_eq!(vm.eval(ScriptMod::new("", "", "after_collision", 0, 0, &"6 * 7", &[])).as_number(),
        Some(42.0), "the VM remains usable after the collision");
}

#[test]
fn a_cold_name_cache_reads_every_script_key_through_each_refusal() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let script = r#"let o = {}
        for i in 0..2000 { o["name_cache_key_" + i] = i }
        let keys = []
        for k, v in o { keys.push(k) }
        keys"#;
    let keys = vm.eval(ScriptMod::new("", "", "mint_keys", 0, 0, &script, &[]))
        .as_array().expect("the script returned the keys it minted");
    let mut names = Vec::new();
    for i in 0..vm.heap().array_len(keys) {
        let id = vm.heap().array_index(keys, i, NoTrap).as_id().expect("a plain object's string key iterates as an id");
        names.push((id, format!("name_cache_key_{i}")));
    }
    assert_eq!(names.len(), 2000, "each script store minted a distinct property");

    let read = |fail| std::thread::scope(|scope| scope.spawn(|| {
        // Initialize this thread's cache before arming. None of the tested
        // names have been read here, even if an earlier trial filled its cache.
        assert_eq!(LiveId::lut_name_len(LiveId::from_str("buffer")), "buffer".len());
        let mut bytes = vec![0; names.iter().map(|(_, s)| s.len()).max().unwrap()];
        let mut matched = true;
        let requests = requests_during(fail, || {
            for (id, spelling) in &names {
                let len = LiveId::lut_name_len(*id);
                let copied = LiveId::lut_name_copy(*id, &mut bytes);
                matched &= len == spelling.len() && bytes.get(..copied) == Some(spelling.as_bytes());
            }
        });
        assert!(matched, "skipping a cache fill must preserve every name's length and bytes at {fail:?}");
        requests
    }).join().unwrap());
    let requests = read(None);
    assert!(requests > names.len(), "the cold read grew the cache map as well as copying the names");
    for k in 0..=requests {
        let observed = read(Some(k));
        assert!(k == requests || observed > k, "the read reached refusal {k}");
    }
}

#[test]
fn refusing_the_printed_name_cache_preserves_every_property_name() {
    // Each trial starts on a fresh thread with its own cache. Warm a few thousand
    // short names up to an actual cache-map grow. This leaves both that grow and
    // the spelling copy in the fault sweep without thousands of redundant runs.
    fn trial(fail: Option<usize>) -> usize {
        let mut host = ScriptVmHost::new((), ());
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        let probe = vm.new_module(id!(name_probe));
        vm.add_method(probe, id_lut!(begin), script_args_def!(), |_, _| {
            ALLOCATIONS.set(Some(0));
            NIL
        });
        let scan = vm.eval(ScriptMod::new("", "", "print_names", 0, 0,
            &r#"fn(o) {
                let count = 0
                mod.name_probe.begin()
                for k, v in o {
                    if ("" + k) != v { return -1 }
                    count += 1
                }
                return count
            }"#, &[]));
        let _scan = vm.heap_mut().new_fn_ref(scan.as_object().expect("the key walk compiled"));
        let object = vm.heap_mut().new_object();
        let _root = vm.heap_mut().new_object_ref(object);
        let mut keys = 0;
        for _ in 0..8192 {
            let spelling = format!("p{keys}");
            let key = vm.heap_mut().new_string_from_str(&spelling);
            vm.heap_mut().set_value_def(object, key, key);
            keys += 1;
            if keys == 1 {
                assert_eq!(vm.call(scan, &[object.into()]).as_number(), Some(1.0));
                ALLOCATIONS.set(None);
            }
            REFUSED.set(0);
            ARMED.set(keys >= 2000);
            let len = LiveId::lut_name_len(LiveId::from_str(&spelling));
            ARMED.set(false);
            assert_eq!(len, spelling.len(), "cache refusal still returns the table's name");
            if REFUSED.get() != 0 { break; }
        }
        assert_ne!(REFUSED.get(), 0, "the prepared keys reached a cache-map grow");

        vm.capture_errors();
        REFUSE_FROM.set(fail);
        let result = vm.call(scan, &[object.into()]);
        REFUSE_FROM.set(None);
        let count = ALLOCATIONS.replace(None).expect("the script armed the scan");
        if let Some(refusal) = vm.heap_mut().take_memory_refusal() {
            assert_eq!(result, ScriptValue::script_err_limit(refusal.ip), "only a VM memory error can end the walk early");
            assert!(vm.heap_mut().take_memory_refusal().is_none(), "the walk reported the refusal once");
        } else {
            assert_eq!(result.as_number(), Some(keys as f64), "every key printed with its stored spelling at {fail:?}");
        }
        assert!(vm.take_errors().is_empty(), "a failed cache fill produced a diagnostic");
        count
    }

    let run = |fail| std::thread::spawn(move || trial(fail)).join().unwrap();
    let requests = run(None);
    assert!(requests >= 2, "the cold name needs both a cache grow and a text copy");
    for k in 0..=requests { run(Some(k)); }
}
