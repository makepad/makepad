//! Element kernels' GPU route (`ElementRoute`): a compute program's `mod.kernel` object as its
//! own compute kernel for every target, its buffers by the kernel's names, or the reason it has
//! none.
#![cfg(feature = "vm")]

use makepad_script::shader_ir_compute::{CompiledKernel, ComputeTarget, KernelCode};
use makepad_script::*;
use makepad_script_compute::element_route::{ElementRoute, ELEMENT_ARGS, ELEMENT_WORKGROUP};
use makepad_script_compute::vm_kernel;
use makepad_script_compute::Backend;

fn route(code: &str) -> ElementRoute {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm_kernel::script_mod(&mut vm, &[]);
    let v = vm.eval(ScriptMod::new("", "", "doc.splash", 0, 0, code, &[]));
    assert!(vm.take_errors().is_empty());
    let obj = v.as_object().expect("the document's kernel object");
    let _keep = vm.new_object_ref(obj);
    let k = vm_kernel::from_object(&vm, obj).unwrap();
    let kernel = vm_kernel::compile(&vm, &k, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    ElementRoute::new(&kernel, 64)
}

fn compiled(route: &ElementRoute, target: ComputeTarget) -> CompiledKernel {
    route.compile(target).view().to_kernel().unwrap_or_else(|e| panic!("{target:?}: {e}"))
}

fn spirv(words: &[u32]) {
    #[cfg(target_os = "linux")]
    {
        let file = std::env::temp_dir().join(format!("element-route-{}-{}.spv", std::process::id(), words.len()));
        std::fs::write(&file, words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>()).unwrap();
        let out = std::process::Command::new("spirv-val").args(["--target-env", "vulkan1.2"]).arg(&file).output().expect("spirv-val required");
        std::fs::remove_file(file).unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
    #[cfg(not(target_os = "linux"))]
    assert!(!words.is_empty());
}

#[test]
fn an_element_kernel_compiles_for_every_target_with_its_buffers_by_name() {
    // a record of two values through views of its stride, a length, the seed and a parameter
    let r = route(
        "use mod.pod.*\nuse mod.kernel.*\n{label: \"axpy\" x: input(f32) y: output(f32) lo: output(f32, 2, 0, \"pair\") hi: output(f32, 2, 1, \"pair\") a: param(2.0) \
         element: fn(i) { y[i] = y[i] + a * x[i + 1] lo[i] = float(len(x)) hi[i] = float(seed + count) }}",
    );
    for target in [ComputeTarget::Spirv, ComputeTarget::Msl, ComputeTarget::Hlsl] {
        let k = compiled(&r, target);
        assert_eq!(k.layout.workgroup, [ELEMENT_WORKGROUP, 1, 1]);
        let names: Vec<(LiveId, bool)> = k.layout.buffers.iter().map(|b| (b.name, b.writable)).collect();
        for (name, writes) in [(id!(x), false), (id!(y), true), (id!(pair), true), (LiveId::from_str(ELEMENT_ARGS), false)] {
            assert!(names.contains(&(name, writes)), "{target:?}: {name} ({writes}) in {names:?}");
        }
        assert_eq!(names.len(), 4, "{target:?}: one storage buffer per named buffer, and the args");
        assert!(k.layout.slots.iter().any(|s| s.name == id!(dispatch_range)), "{target:?}: the runtime's dispatch range");
        match &k.code {
            KernelCode::Spirv(words) => spirv(words),
            KernelCode::Msl(text) | KernelCode::Hlsl(text) => assert!(text.contains("kernel_main")),
        }
    }
}

#[test]
fn an_element_kernel_the_gpu_cannot_run_says_why() {
    let f64 = route("use mod.pod.*\nuse mod.kernel.*\n{label: \"wide\" x: input(@f64) y: output(@f64) element: fn(i) { y[i] = x[i] * x[i] }}");
    let why = |r: &ElementRoute| r.compile(ComputeTarget::Spirv).view().to_kernel().err().unwrap_or_default();
    assert!(why(&f64).contains("f64"), "{}", why(&f64));
    let shared = route("use mod.pod.*\nuse mod.kernel.*\n{label: \"spread\" y: output(f32) element: fn(i) { y[i * 2] = 1.0 }}");
    assert!(why(&shared).contains("other elements"), "{}", why(&shared));
}
