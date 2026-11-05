//! A kernel's trace (`lower::trace`): what a host reads of a kernel's lowering. It knows nothing of
//! what the kernel is for: helpers' calls, their arguments and values, and one element's run.

use makepad_script_compute::kernel::{compile, trace};
use makepad_script_compute::lower::trace::{TraceArgKind, TraceDefKind};
use makepad_script_compute_sched::kernel::KernelExt;

const KERNEL: &str = r#"
    let xs = input(vec3)
    let o = output(f32)
    let amp = param(2.0)
    fn twice(x) {
        let y = x + x
        return y
    }
    fn first(q) { return q.x }
    fn element(i) {
        let p = xs[i]
        let d = twice(p.x + amp)
        let r = sdf_round(length(p), 0.5)
        o[i] = sdf_round(r, d) + first(p)
    }
"#;

#[test]
fn a_trace_reads_every_call_and_its_values() {
    let t = trace(KERNEL).unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    assert_eq!(view.params, ["amp"]);
    let mut values = vec![0; view.values];
    t.run(&[3.0], "xs", &[1.0, 2.0, 2.0], &mut values);
    let value = |v: u32| f32::from_bits(values[v as usize]);
    let names: Vec<&str> = view.calls.iter().map(|c| c.name).collect();
    assert_eq!(names, ["twice", "sdf_round", "sdf_round", "first"], "every call, in order");

    let twice = &view.calls[0];
    assert!(!twice.library, "the program's own");
    assert_eq!(twice.params, ["x"]);
    assert_eq!(twice.args[0].kind, TraceArgKind::Float);
    assert_eq!(value(twice.args[0].lanes[0]), 4.0);
    assert_eq!(value(twice.value), 8.0);

    let inner = &view.calls[1];
    assert!(inner.library, "the prelude's");
    assert_eq!(inner.params, ["d", "r"]);
    assert_eq!(value(inner.args[0].lanes[0]), 3.0);
    assert_eq!((inner.args[1].kind, inner.args[1].literal), (TraceArgKind::Literal, 0.5));
    assert_eq!(value(inner.value), 2.5);

    let outer = &view.calls[2];
    assert_eq!(outer.args[0].lanes, [inner.value], "a let names its value");
    assert_eq!(outer.args[1].lanes, [twice.value]);
    assert_eq!(value(outer.value), -5.5);

    let first = &view.calls[3];
    assert_eq!(first.args[0].kind, TraceArgKind::Vector);
    assert_eq!(first.args[0].lanes.len(), 3);
    assert_eq!(value(first.value), 1.0);
}

/// A helper's value leads back through the arithmetic that made it, down to where it was read.
#[test]
fn a_trace_defines_what_a_call_is_made_of() {
    let t = trace(KERNEL).unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let def = |v: u32| *view.defs.iter().find(|d| d.val == v).unwrap_or_else(|| panic!("value {v} has no definition"));
    let twice = &view.calls[0];
    let doubled = def(twice.value);
    assert_eq!(doubled.kind, TraceDefKind::Add);
    assert!(doubled.float);
    assert_eq!(doubled.argc, 2);
    assert_eq!(doubled.args[0], twice.args[0].lanes[0], "a helper's parameter names its argument");
    let sum = def(twice.args[0].lanes[0]);
    assert_eq!(sum.kind, TraceDefKind::Add);
    assert_eq!(def(sum.args[0]).kind, TraceDefKind::Memory, "the input");
    assert_eq!(def(sum.args[1]).kind, TraceDefKind::Context, "the parameter");
}

/// A value made in a branch is no value of the blocks around it: the call is noted, its value has no
/// definition there.
#[test]
fn a_call_in_a_branch_has_no_definition_in_the_view() {
    let t = trace("let o = output(f32)\nfn one(x) { return x + 1.0 }\nfn element(i) { let v = 0.0\n if i > 0 { v = one(float(i)) }\n o[i] = v }").unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let call = view.calls.iter().find(|c| c.name == "one").expect("the call in the branch");
    assert!(view.defs.iter().all(|d| d.val != call.value));
}

/// Tracing changes no answer: the run reads what the compiled kernel computes.
#[test]
fn a_traces_run_agrees_with_the_compiled_kernel() {
    let t = trace(KERNEL).unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let mut values = vec![0; view.values];
    let point = [1.0f32, 2.0, 2.0];
    t.run(&[3.0], "xs", &point, &mut values);
    let value = |v: u32| f32::from_bits(values[v as usize]);

    let k = compile(KERNEL).unwrap_or_else(|e| panic!("{e:?}"));
    let mut out = [0.0f32];
    let mut call = k.call();
    call.set_param("amp", 3.0);
    call.input("xs", &point).unwrap();
    call.output("o", &mut out).unwrap();
    call.run(1).unwrap();
    assert_eq!(out[0], value(view.calls[2].value) + value(view.calls[3].value));
}

#[test]
fn a_program_that_does_not_lower_has_no_trace() {
    assert!(trace("fn element(i) { o[i] = nothing(i) }").is_err());
}
