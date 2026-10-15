//! Kernels made from Splash values (`vm_kernel`): the entry is the VM's fn
//! object and what it reaches is found through the VM's scopes, not by
//! reading document text.
#![cfg(feature = "vm")]

use makepad_script_compute_sched::kernel::KernelExt;
use makepad_script::*;
use makepad_script_compute::kernel::MathMode;
use makepad_script_compute::vm_kernel::{self, Decl, Entry, Record, VmKernel};
use makepad_script_compute::Backend;

fn doc(code: &str) -> (ScriptVm<'static>, ScriptObject) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", "doc.splash", 0, 0, &code, &[]));
    assert!(vm.take_errors().is_empty());
    let obj = v.as_object().expect("the document's kernel object");
    let _ = vm.new_object_ref(obj);
    (vm, obj)
}

fn field(vm: &ScriptVm, obj: ScriptObject, name: LiveId) -> ScriptObject {
    vm.value(obj, name).as_object().expect("a fn field")
}

#[test]
fn entry_reaches_document_fns_and_constants_through_the_vm() {
    let (vm, k) = doc(
        "use mod.pod.*\nlet R = 3.5\nlet OFF = vec3(1.0, 2.0, 3.0)\nlet scale = fn(x) { return x * R }\n\
         fn bump(y) { return scale(y) + 1.0 }\n\
         let K = {count: 4 element: fn(i) { let f = float(i) pos[i] = vec3(bump(f), f, 0.0) + OFF * amp }}\nK",
    );
    let kernel = VmKernel {
        decls: vec![
            Decl::Output { name: "pos".into(), ty: "vec3".into(), stride: None, offset: None, buffer: None },
            Decl::Param { name: "amp".into(), default: 2.0, range: None },
        ],
        entry: Entry::Element,
        entry_fn: field(&vm, k, id!(element)),
        math: MathMode::Portable,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    // `bump`, `scale`, `R` and `OFF` are found through the VM and compiled in.
    let compiled = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let mut out = vec![0f32; 12];
    let mut call = compiled.call();
    call.output("pos", &mut out).unwrap();
    call.run(4).unwrap();
    drop(call);
    for i in 0..4 {
        let f = i as f32;
        assert_eq!(&out[i * 3..i * 3 + 3], &[f * 3.5 + 1.0 + 2.0, f + 4.0, 6.0], "element {i}");
    }
}

#[test]
fn errors_name_the_document_line() {
    let (vm, k) = doc("let helper = fn(x) {\n    return x + undefined_thing\n}\nlet K = {element: fn(i) { out[i] = helper(1.0) }}\nK");
    let kernel = VmKernel {
        decls: vec![Decl::EmitBuffer { name: "unused".into(), record: Record::Words(1), capacity: 1, buffer: None },
                    Decl::Output { name: "out".into(), ty: "f32".into(), stride: None, offset: None, buffer: None }],
        entry: Entry::Element,
        entry_fn: field(&vm, k, id!(element)),
        math: MathMode::Fast,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    let errors = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).err().expect("an error");
    assert!(errors.iter().any(|e| e.message.contains("doc.splash:2:")), "{errors:?}");
}

#[test]
fn a_kernel_object_reads_back_from_the_vm() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.capture_errors();
    use makepad_script_compute::boundary::{LayoutFieldView, LayoutView};
    use makepad_script_compute::kernel::FieldTy;
    let layout = LayoutView { name: "Dot", stride: 4, fields: &[LayoutFieldView { name: "pos", ty: FieldTy::Vec3, offset: 0 }, LayoutFieldView { name: "size", ty: FieldTy::F32, offset: 3 }] };
    vm_kernel::script_mod(&mut vm, std::slice::from_ref(&layout));
    let v = vm.eval(ScriptMod::new("", "", "doc.splash", 0, 0, "use mod.pod.*\nuse mod.kernel.*\nlet GAP = 2.0\n{count: 3 dots: output(Dot) h: input(f32) amp: param(0.5, 0.0, 1.0) math: \"portable\" element: fn(i) { dots[i].pos = vec3(float(i) * GAP, h[i], 0.0) dots[i].size = amp }}", &[]));
    assert!(vm.take_errors().is_empty());
    let obj = v.as_object().unwrap();
    let _keep = vm.new_object_ref(obj);
    let k = vm_kernel::from_object(&vm, obj).unwrap();
    assert_eq!(k.entry, Entry::Element);
    assert_eq!(k.math, MathMode::Portable);
    assert_eq!(k.decls.len(), 3);
    let compiled = vm_kernel::compile(&vm, &k, &[layout], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let h = [10.0f32, 20.0, 30.0];
    let mut dots = vec![0f32; 12];
    let mut call = compiled.call();
    call.input("h", &h).unwrap();
    call.output("dots", &mut dots).unwrap();
    call.run(3).unwrap();
    drop(call);
    assert_eq!(dots, vec![0.0, 10.0, 0.0, 0.5, 2.0, 20.0, 0.0, 0.5, 4.0, 30.0, 0.0, 0.5]);
}

#[test]
fn closures_and_colours_compile_from_the_vm_tokens() {
    let (vm, k) = doc(
        "use mod.pod.*\nlet TINT = #ff8000\nlet NEG = -2.5\nlet sq = |x| x * x\nlet add = |a, b| { return a + b }\nlet one = || 1.0\n\
         let K = {element: fn(i) { let f = float(i) out[i] = vec4(add(sq(f), one()), NEG, 0.0, 0.0) + TINT }}\nK",
    );
    let kernel = VmKernel {
        decls: vec![Decl::Output { name: "out".into(), ty: "vec4".into(), stride: None, offset: None, buffer: None }],
        entry: Entry::Element,
        entry_fn: field(&vm, k, id!(element)),
        math: MathMode::Fast,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    let compiled = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let mut out = vec![0f32; 12];
    let mut call = compiled.call();
    call.output("out", &mut out).unwrap();
    call.run(3).unwrap();
    drop(call);
    for i in 0..3 {
        let f = i as f32;
        assert_eq!(&out[i * 4..i * 4 + 4], &[f * f + 1.0 + 1.0, -2.5 + 128.0 / 255.0, 0.0, 1.0], "element {i}");
    }
}

#[test]
fn a_kernel_made_from_values_is_recorded_for_build_tools() {
    let (vm, k) = doc("let K = {element: fn(i) { rec[i] = float(i) * 7.25 }}\nK");
    let kernel = VmKernel {
        decls: vec![Decl::Output { name: "rec".into(), ty: "f32".into(), stride: None, offset: None, buffer: None }],
        entry: Entry::Element,
        entry_fn: field(&vm, k, id!(element)),
        math: MathMode::Fast,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    makepad_script_compute::kernel::record_compiled(true);
    let compiled = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let recorded = makepad_script_compute::kernel::take_recorded();
    let key = makepad_script_compute::kernel::program_key(compiled.program());
    assert!(recorded.iter().any(|r| makepad_script_compute::kernel::program_key(r.kernel.program()) == key && r.source.is_empty()));
}

#[test]
fn a_kernel_object_compiled_with_a_drifted_layout_is_an_error() {
    use makepad_script_compute::boundary::{register_layout, LayoutFieldView, LayoutView};
    use makepad_script_compute::kernel::FieldTy;
    const GOOD: &[LayoutFieldView<'static>] = &[LayoutFieldView { name: "pos", ty: FieldTy::Vec3, offset: 0 }];
    const DRIFTED: &[LayoutFieldView<'static>] = &[LayoutFieldView { name: "pos", ty: FieldTy::Vec3, offset: 1 }];
    let good = LayoutView { name: "ObjDriftProbe", stride: 4, fields: GOOD };
    let drifted = LayoutView { name: "ObjDriftProbe", stride: 4, fields: DRIFTED };
    assert!(register_layout(&good));
    let (vm, k) = doc("let K = {element: fn(i) { out[i].pos = vec3(1.0, 2.0, 3.0) }}\nK");
    let kernel = |vm: &ScriptVm| VmKernel {
        decls: vec![Decl::Output { name: "out".into(), ty: "ObjDriftProbe".into(), stride: None, offset: None, buffer: None }],
        entry: Entry::Element,
        entry_fn: field(vm, k, id!(element)),
        math: MathMode::Fast,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    assert!(vm_kernel::compile(&vm, &kernel(&vm), &[good], Backend::Interp, &[]).is_ok());
    let errors = vm_kernel::compile(&vm, &kernel(&vm), &[drifted], Backend::Interp, &[]).err().expect("the drifted table must not compile");
    assert!(errors.iter().any(|e| e.message.contains("`ObjDriftProbe` is not the registered one")), "{errors:?}");
}
