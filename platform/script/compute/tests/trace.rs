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

/// A let assigned again only in line (whole assignments in its own block, outside any branch,
/// loop or function) names each value in turn, as a chain of lets would, and the run still reads
/// what the compiled kernel computes; one assigned in a branch stays a variable, read as memory.
#[test]
fn a_let_assigned_in_line_names_each_value() {
    let src = "let xs = input(vec3)\nlet o = output(f32)\nfn element(i) {\n    let p = xs[i]\n    let d = sdf_round(length(p), 0.5)\n    d = sdf_round(d, 0.25)\n    d = min(d, p.x)\n    o[i] = sdf_round(d, 0.125)\n}";
    let t = trace(src).unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let def = |v: u32| *view.defs.iter().find(|d| d.val == v).unwrap_or_else(|| panic!("value {v} has no definition"));
    let rounds: Vec<_> = view.calls.iter().filter(|c| c.name == "sdf_round").collect();
    assert_eq!(rounds.len(), 3);
    assert_eq!(rounds[1].args[0].lanes, [rounds[0].value], "the first value");
    let least = def(rounds[2].args[0].lanes[0]);
    assert_eq!(least.kind, TraceDefKind::Min, "the third value, the lesser of two");
    assert_eq!(least.args[0], rounds[1].value, "the second value");

    let point = [1.0f32, 2.0, 2.0];
    let mut values = vec![0; view.values];
    t.run(&[], "xs", &point, &mut values);
    let k = compile(src).unwrap_or_else(|e| panic!("{e:?}"));
    let mut out = [0.0f32];
    let mut call = k.call();
    call.input("xs", &point).unwrap();
    call.output("o", &mut out).unwrap();
    call.run(1).unwrap();
    assert_eq!(out[0], f32::from_bits(values[rounds[2].value as usize]));
    assert_eq!(out[0], 0.875);

    let t = trace("let o = output(f32)\nfn element(i) {\n    let v = sdf_round(float(i), 0.5)\n    if i > 0 { v = sdf_round(v, 0.25) }\n    o[i] = sdf_round(v, 0.125)\n}").unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let last = view.calls.iter().rev().find(|c| c.name == "sdf_round").expect("the last call");
    let read = view.defs.iter().find(|d| d.val == last.args[0].lanes[0]).expect("its argument's definition");
    assert_eq!(read.kind, TraceDefKind::Memory, "a variable");
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

/// An array argument is noted element by element: a local array written just before the call
/// reads as the values written, each leading back to what made it.
#[test]
fn a_trace_reads_an_array_argument_by_its_elements() {
    let src = "let xs = input(vec3)
let o = output(f32)
let amp = param(2.0)
fn nearest(p, balls) {
    let best = 1000.0
    for k in 0..len(balls) { best = min(best, length(p - balls[k].xyz) - balls[k].w) }
    best
}
fn element(i) {
    let p = xs[i]
    let fixed = [vec4(0.0, 0.0, 0.0, 1.0), vec4(amp, 1.0, 0.0, 0.5)]
    o[i] = nearest(p, fixed) + nearest(p, [vec4(p.x, 0.0, 0.0, 1.0)])
}";
    let t = trace(src).unwrap_or_else(|e| panic!("{e:?}"));
    let view = t.view();
    let mut values = vec![0; view.values];
    t.run(&[3.0], "xs", &[1.0, 2.0, 2.0], &mut values);
    let value = |v: u32| f32::from_bits(values[v as usize]);
    let def = |v: u32| *view.defs.iter().find(|d| d.val == v).unwrap_or_else(|| panic!("value {v} has no definition"));
    let calls: Vec<_> = view.calls.iter().filter(|c| c.name == "nearest").collect();
    assert_eq!(calls.len(), 2);

    let fixed = &calls[0].args[1];
    assert_eq!(fixed.kind, TraceArgKind::Array);
    let lanes: Vec<f32> = fixed.lanes.iter().map(|v| value(*v)).collect();
    assert_eq!(lanes, [0.0, 0.0, 0.0, 1.0, 3.0, 1.0, 0.0, 0.5], "every element's lanes, in order");
    assert_eq!(def(fixed.lanes[0]).kind, TraceDefKind::Const, "a written constant reads as itself");
    assert_eq!(def(fixed.lanes[4]).kind, TraceDefKind::Context, "the parameter");
    assert!(view.defs.iter().any(|d| d.val == calls[0].value), "the helper's value is the block's");

    let moving = &calls[1].args[1];
    assert_eq!(moving.kind, TraceArgKind::Array);
    assert_eq!(moving.lanes.len(), 4);
    assert_eq!(value(moving.lanes[0]), 1.0);
    assert_eq!(def(moving.lanes[0]).kind, TraceDefKind::Memory, "the point's x, read from the input");
}

#[test]
fn a_program_that_does_not_lower_has_no_trace() {
    assert!(trace("fn element(i) { o[i] = nothing(i) }").is_err());
}
