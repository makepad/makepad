use makepad_script::*;
use std::time::Duration;

fn module(code: &str) -> ScriptModView<'_> {
    ScriptModView { file: "entry-policy.splash", code, ..ScriptModView::empty() }
}

#[test]
fn hard_limits_stop_only_the_guest_at_each_entry() {
    let mut host = ScriptVmHost::new((), ());
    let mut host = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    for (limit, body) in [
        (Limit::Fuel, "for i in 0..1000 { i }"),
        (Limit::CallFrames, "fn one() { 1 }\none()"),
        (Limit::OperandStack, "1 + 2"),
        (Limit::RetainedHeap, "values[65536] = 1"),
        (Limit::Allocation, "values[65536] = 1"),
        (Limit::StringBytes, "let s = \"abcdefgh\"\ns + s"),
    ] {
        for entry in ["load", "stream", "callback", "resume"] {
            // Each stopped VM is terminal. The same host handles every case.
            let mut guest_host = ScriptVmHost::new((), ());
            let mut vm = ScriptVm { host: guest_host.host_context(), bx: Box::new(ScriptVmBase::new()) };
            let array = vm.heap_mut().new_array();
            vm.set_injected_global(id!(values), array.into());
            let closure = vm.eval(ScriptMod::new("", "", "fixture.splash", 0, 0, &format!("fn() {{ {body} }}\n;"), &[]));
            assert!(closure.is_object(), "{entry} {limit}: the callback fixture compiled");
            vm.heap_mut().reconcile_heap_bytes();
            let mut policy = ScriptVmPolicy::untrusted();
            match limit {
                Limit::Fuel => policy.fuel = Some(32),
                Limit::CallFrames => policy.call_frames = Some(1),
                Limit::OperandStack => policy.stack_values = Some(1),
                Limit::RetainedHeap => policy.retained_heap_bytes = Some(vm.heap().accounted_heap_bytes() + (64 << 10)),
                Limit::Allocation => policy.allocation_bytes = Some(64 << 10),
                Limit::StringBytes => policy.string_bytes = Some(8),
                _ => unreachable!(),
            }
            vm.bx.set_policy(policy).expect("the fresh guest accepts its policy");
            let code = format!("{body}\n;");
            let mut call = ScriptCallReq::new(CALL_ARGS, closure);
            let result = match entry {
                "load" => vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module(&code))),
                "stream" => {
                    let mut req = ScriptEvalReq::new(EVAL_APPEND, module(""));
                    req.code = &code;
                    vm.load_entry(&mut req)
                }
                "callback" => vm.call_entry(&mut call),
                _ => {
                    vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 1)));
                    let started = vm.call_entry(&mut call);
                    assert!(started.is_ok(), "{limit}: the pre-limit slice starts: {started:?}");
                    assert!(vm.run_yielded(), "{limit}: the callback yielded before doing the limited work");
                    vm.set_run_budget(None);
                    vm.resume_entry()
                }
            };
            let reason = Poison::Limit(limit);
            assert_eq!(result, Err(&reason), "{entry}: the failed entry reports {limit}");
            assert_eq!(vm.bx.poisoned(), Some(&reason), "{entry} {limit}: the first reason persists");
            let policy = vm.bx.policy();
            let usage = (policy.last_entry(), policy.total());
            assert!(usage.1.fuel > 0, "{entry} {limit}: executed instructions are counted");
            assert!(usage.1.bytes > 0, "{entry} {limit}: charged bytes are counted");
            let bodies = vm.bx.body_count();
            assert_eq!(vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module("1"))), Err(&reason), "a stopped load returns the original reason");
            assert_eq!(vm.call_entry(&mut call), Err(&reason), "a stopped callback returns the original reason");
            assert_eq!(vm.resume_entry(), Err(&reason), "a stopped resume returns the original reason");
            assert_eq!(vm.bx.set_policy(ScriptVmPolicy::default()), Err(&reason), "installing a default policy cannot restart the box");
            assert_eq!(vm.bx.body_count(), bodies, "a stopped load never installs a body");
            let policy = vm.bx.policy();
            assert_eq!((policy.last_entry(), policy.total()), usage, "{entry} {limit}: rejected entries did no work");
            assert_eq!(host.vm_eval(&mut ScriptEvalReq::new(EVAL_SOURCE, module("41 + 1\n;"))).as_number(), Some(42.0), "the host still runs after {entry} hit {limit}");
        }
    }
}

#[test]
fn streaming_and_native_callbacks_keep_the_first_uncaught_error() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.set_policy(ScriptVmPolicy::untrusted()).expect("install the untrusted policy");
    let mut req = ScriptEvalReq::new(EVAL_APPEND, module(""));
    req.code = "try mod.std.assert(false) catch 42\n;";
    assert_eq!(vm.load_entry(&mut req).unwrap().as_number(), Some(42.), "a caught error leaves the box running");
    req.code = "[1].map(fn(v) { mod.std.assert(false, \"callback stopped\") })\n42\n;";
    assert!(matches!(vm.load_entry(&mut req), Err(Poison::Error(s)) if s.contains("callback stopped")),
        "a nested uncaught error reaches the streaming caller with its reason: {:?}", vm.bx.poisoned());
}

#[test]
fn a_guests_gc_calls_only_ask_and_leave_what_they_touch_collectable() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let code = "fn() {\n    for i in 0..2000 {\n        mod.gc.set_static({n: i})\n        mod.gc.run()\n        mod.gc.run_status()\n    }\n    1\n}\n;";
    let closure = vm.eval(ScriptMod::new("", "", "fixture.splash", 0, 0, code, &[]));
    let _keep = vm.new_object_ref(closure.as_object().expect("the fixture compiled"));
    vm.bx.set_policy(ScriptVmPolicy::untrusted()).expect("install the untrusted policy");
    vm.gc();
    let before = vm.heap().gc_live_len();
    let mut call = ScriptCallReq::new(CALL_ARGS, closure);
    assert_eq!(vm.call_entry(&mut call).ok().and_then(|v| v.as_number()), Some(1.), "the loop runs within one entry's fuel");
    assert!(vm.bx.heap_mut().take_gc_request(), "the guest's collections are one request to its host");
    assert!(!vm.bx.heap_mut().take_gc_request(), "taking the request clears it");
    vm.gc();
    assert_eq!(vm.heap().gc_live_len(), before, "set_static under a policy leaves the values collectable");
}

#[test]
fn debug_logging_from_a_guest_never_reaches_the_host_log() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static MESSAGES: AtomicUsize = AtomicUsize::new(0);
    fn observe(message: &str, _: makepad_error_log::LogLevel) {
        if message.contains("entry_policy_log_probe") || message.contains("proto_type_index=") {
            MESSAGES.fetch_add(1, Ordering::Relaxed);
        }
    }
    struct Tap;
    impl Drop for Tap {
        fn drop(&mut self) { makepad_error_log::set_log_tap(None); }
    }
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let log = "mod.std.log(\"entry_policy_log_probe\")\nmod.gc.dump_tag({})\n7\n;";
    makepad_error_log::set_log_tap(Some(observe));
    let _tap = Tap;
    let before = MESSAGES.load(Ordering::Relaxed);
    assert_eq!(vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module(log))).unwrap().as_number(), Some(7.), "a stock VM can log and continue");
    let after = MESSAGES.load(Ordering::Relaxed);
    assert_eq!(after - before, 2, "the host log received the stock log and GC tag");
    vm.bx.set_policy(ScriptVmPolicy::untrusted()).expect("install the guest policy");
    assert_eq!(vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module(log))).unwrap().as_number(), Some(7.), "the log native returns normally with output disabled");
    let std = vm.value(vm.modules(), id!(std)).as_object().unwrap();
    let muted = vm.value(std, id!(log));
    let args = [123456_u32.into()];
    vm.call(muted, &args);
    let call_bytes = vm.bx.policy().last_entry().bytes;
    for name in [id!(print), id!(println)] {
        let print = vm.value(std, name);
        assert!(vm.call(print, &args).is_nil(), "the silenced print native returns nil");
        assert_eq!(vm.bx.policy().last_entry().bytes, call_bytes, "the print native did not allocate to format its output");
    }
    let opcode = "~\"entry_policy_log_probe\"\n;";
    assert!(matches!(vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module(opcode))), Err(Poison::Error(_))), "the log operator reports the disabled operation to the guest");
    assert_eq!(MESSAGES.load(Ordering::Relaxed), after, "neither guest log route reached the host log");
}

#[test]
fn nested_policy_and_document_budgets_charge_the_same_allocation_once() {
    fn copy(vm: &mut ScriptVm, text: &str) {
        let value = ScriptValue::from_inline_string(text).expect("inline test string");
        let _ = vm.heap_mut().cast_to_owned_string(value, "copying test text");
    }

    let mut host = ScriptVmHost::new((), ());
    for document_outside in [true, false] {
        for (document_bytes, policy_bytes) in [(64, 64), (6, 64), (64, 6)] {
            let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
            let mut policy = ScriptVmPolicy::untrusted();
            policy.allocation_bytes = Some(policy_bytes);
            vm.bx.set_policy(policy).expect("fresh guest accepts its policy");
            let limits = EvalLimits { heap_bytes: document_bytes, ..EvalLimits::DOC_FRAME };
            let mut document = EvalReport::default();
            if document_outside && policy_bytes == 64 && document_bytes == 64 {
                let (_, report) = vm.eval_bounded(&limits, |vm| {
                    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        vm.host_entry(|vm| vm.with_heap_allocation_limit(3, |vm| {
                            copy(vm, "abc");
                            panic!("test native unwound");
                        }).0)
                    }));
                    assert!(panic.is_err(), "the native panic reached its host boundary");
                    copy(vm, "def");
                });
                assert_eq!(report.heap.allocated_bytes, 6, "the parent was restored and retained the unwound scope's bytes");
                assert_eq!(vm.bx.policy().last_entry().bytes, 3, "the unwound native's allocation was counted once");
            }
            let before = vm.heap().accounted_heap_bytes();
            if document_outside {
                (_, document) = vm.eval_bounded(&limits, |vm| {
                    copy(vm, "abc");
                    vm.host_entry(|vm| { copy(vm, "def"); copy(vm, "ghijk"); NIL })
                });
            } else {
                vm.host_entry(|vm| {
                    copy(vm, "abc");
                    (_, document) = vm.eval_bounded(&limits, |vm| {
                        copy(vm, "def");
                        copy(vm, "ghijk");
                    });
                    NIL
                });
            }
            let success = document_bytes == 64 && policy_bytes == 64;
            let outer = if success { 11 } else { 6 };
            let inner = outer - 3;
            assert_eq!(vm.heap().accounted_heap_bytes() - before, outer, "the retained heap sees each accepted charge once");
            assert_eq!(vm.bx.policy().last_entry().bytes, if document_outside {inner} else {outer}, "the policy counts every allocation inside its scope");
            assert_eq!(document.heap.allocated_bytes, if document_outside {outer} else {inner}, "the bounded document counts every allocation inside its scope");
            assert_eq!(vm.bx.poisoned(), (!success).then_some(&Poison::Limit(Limit::Allocation)), "the tighter allocation cap stops the guest in either nesting order");
        }
    }
}

#[test]
fn equality_reports_the_first_counter_to_refuse_work() {
    let mut host = ScriptVmHost::new((), ());
    let bytes = vec![0; equality::MAX_EQUALITY_WORK];
    let mut compare = |fuel, expected| {
        let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
        for name in [id!(left), id!(right)] {
            let array = vm.heap_mut().new_array_from_slice_u8(&bytes);
            vm.set_injected_global(name, array.into());
        }
        let mut policy = ScriptVmPolicy::untrusted();
        policy.fuel = Some(fuel);
        vm.bx.set_policy(policy).expect("fresh comparison VM accepts the policy");
        let value = vm.load_entry(&mut ScriptEvalReq::new(EVAL_SOURCE, module("left == right\n;")));
        assert_eq!(value, Err(&Poison::Limit(expected)), "the comparison reports the counter that refused its next unit");
        vm.bx.policy().last_entry().fuel
    };
    let cost = compare(u64::MAX, Limit::EqualityWork);
    // On the last allowed equality unit, the last instruction is spent as
    // well. The next equality unit is refused before requesting more fuel.
    compare(cost, Limit::EqualityWork);
    compare(cost - 1, Limit::Fuel);
}
