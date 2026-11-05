//! Pod types are collected with the object that owns them. A module run again (a design edit
//! re-runs its `script_mod`) defines its pod structs again; before, every pod type's object was
//! a collector root, so each run's types and what they held stayed for good. Now a pod type
//! lives while its owner is reached: from the module, from a pod of the type, from a type
//! built on it, or from a static value.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm
}

// Pod structs, a pod built from them, and a shader that builds one of them and an array (the
// compiler makes the array's type on the heap) compiled on every run.
const MODULE: &str = "use mod.pod.*
use mod.math.*
use mod.shader
let P = struct{a: f32, b: vec4f}
let Q = struct{p: P, n: array{f32 2}}
let sh = {
    vertex_pos: shader.vertex_position(vec4f)
    pixel: shader.fragment_output(0, vec4f)
    vertex: fn() {
        self.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)
    }
    fragment: fn() {
        var p = P(1f, vec4(0.5))
        var arr = array(1f, 2f, 3f)
        self.pixel = p.b * arr[1]
    }
}
[Q(P(1, vec4f(1, 2, 3, 4)), array(4f 5f)), shader.test_compile_draw_source(sh, \"metal\", false)]
";

#[test]
fn a_rerun_modules_pod_types_are_collected_and_kept_pods_keep_theirs() {
    let mut vm = test_vm();
    let mut first_src = None;
    let mut run = |vm: &mut ScriptVm| {
        let out = vm.eval(ScriptMod::new("", "", "pod_types_collected.splash", 0, 0, MODULE, &[]));
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{errors:?}");
        let out = out.as_array().expect("the module returns [pod, shader source]");
        let (pod, src) = (vm.bx.heap.array_index(out, 0, NoTrap), vm.bx.heap.array_index(out, 1, NoTrap));
        assert!(pod.as_pod().is_some(), "the module returns a pod");
        let src = vm.bx.heap.string_with(src, |_, s| s.to_string()).expect("the shader compiles to source");
        assert!(!src.starts_with("ERRORS"), "{src}");
        assert_eq!(&src, first_src.get_or_insert_with(|| src.clone()), "a re-run compiled another shader");
        pod
    };
    // A pod from the first run, kept by a rooted object, and one made static: both outlive
    // every type their module defines again.
    let holder = vm.bx.heap.new_object();
    let _holder_ref = vm.bx.heap.new_object_ref(holder);
    let kept = run(&mut vm);
    vm.bx.heap.set_value_def(holder, id!(kept).into(), kept);
    let pinned = run(&mut vm);
    vm.bx.heap.set_static(pinned);

    let mut warm = (0, 0);
    for n in 0..20 {
        run(&mut vm);
        vm.gc();
        if n == 2 {
            warm = (vm.heap().gc_live_len(), vm.heap().heap_stat(HEAP_STAT_POD_TYPES));
        }
    }
    let now = (vm.heap().gc_live_len(), vm.heap().heap_stat(HEAP_STAT_POD_TYPES));
    assert_eq!(now.1, warm.1, "live pod types grew across re-runs: each run's types were kept");
    assert_eq!(now.0, warm.0, "the live heap grew across re-runs");

    // The kept pods read through their own types, though later runs reused freed slots.
    vm.set_injected_global(id!(kept), kept);
    vm.set_injected_global(id!(pinned), pinned);
    let ok = vm.eval(ScriptMod::new("", "", "pod_types_read.splash", 0, 0, "kept.p.a == 1f && kept.n[1] == 5f && pinned.n[0] == 4f && pinned.p.a == 1f", &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(ok.as_bool(), Some(true), "a kept pod lost its type");
}
