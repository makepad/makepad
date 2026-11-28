//! Kernels made from Splash values (`vm_kernel`): the entry is the VM's fn
//! object and what it reaches is found through the VM's scopes, not by
//! reading document text.
#![cfg(feature = "vm")]

use makepad_script_compute_sched::kernel::KernelExt;
use makepad_script::*;
use makepad_script_compute::kernel::{self, MathMode};
use makepad_script_compute::vm_kernel::{self, Decl, Entry, Record, VmKernel};
use makepad_script_compute::Backend;

static RECORD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Recording {
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl Recording {
    fn start() -> Self {
        let lock = RECORD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        kernel::record_compiled(true);
        Self { _lock: lock }
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        kernel::record_compiled(false);
    }
}

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
        whole_ranges: false,
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
        whole_ranges: false,
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
        whole_ranges: false,
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
        whole_ranges: false,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    let _recording = Recording::start();
    let compiled = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let recorded = makepad_script_compute::kernel::take_recorded();
    let key = makepad_script_compute::kernel::program_key(compiled.program());
    assert!(recorded.iter().any(|r| makepad_script_compute::kernel::program_key(r.kernel.program()) == key && r.source.is_empty()),
        "the value-built kernel was recorded with empty source");
}

#[test]
fn take_recorded_and_wasm_module_round_trip() {
    use makepad_script_compute::boundary::{LayoutFieldView, LayoutView};
    use makepad_script_compute::kernel::{FieldTy, LayoutField};
    use makepad_script_compute::module::Module;

    let layout = LayoutView { name: "Sample", stride: 4, fields: &[
        LayoutFieldView { name: "pos", ty: FieldTy::Vec3, offset: 0 },
        LayoutFieldView { name: "weight", ty: FieldTy::F32, offset: 3 },
    ] };
    let module = Module { path: "lib:weights@1", source: "fn weight(x) { x * 0.25 }" };
    let source = "use lib(\"weights\", \"1\") as w\nlet out = output(Sample)\nfn element(i) { out[i].weight = w.weight(float(i)) }";
    let mut record = {
        let _recording = Recording::start();
        let compiled = kernel::compile_with_modules(source, &[layout], Backend::Interp, std::slice::from_ref(&module))
            .expect("the kernel using the module and record layout compiles");
        let key = kernel::program_key(compiled.program());
        kernel::take_recorded().into_iter().find(|r| kernel::program_key(r.kernel.program()) == key)
            .expect("the compiled kernel was recorded")
    };
    assert_eq!(record.source, source, "the recorded kernel retains its exact source");
    assert_eq!(record.modules.len(), 1, "one module was recorded");
    assert_eq!(record.modules[0].path, module.path, "the module keeps its import path");
    assert_eq!(record.modules[0].text, module.source, "the module keeps its source");
    assert_eq!(record.layouts.len(), 1, "one layout was recorded");
    let saved = &mut record.layouts[0];
    assert_eq!(saved.name, layout.name, "the layout keeps its name");
    assert_eq!(saved.stride, layout.stride, "the layout keeps its stride");
    assert_eq!(saved.fields.len(), layout.fields.len(), "all fields were copied");
    for (field, expected) in saved.fields.iter().zip(layout.fields) {
        assert_eq!((field.name.as_str(), field.ty, field.offset), (expected.name, expected.ty, expected.offset),
            "the recorded field keeps its name, type, offset and position");
    }
    let copy = saved.clone();
    saved.fields.push(LayoutField { name: "extra".into(), ty: FieldTy::I32, offset: 4 });
    assert_eq!(copy.fields.len(), 2, "the cloned layout owns its fields independently");
    record.layouts.push(copy);
    drop(record.layouts);

    let kernels = [record.kernel.as_ref()];
    let (vectorized, flags) = kernel::wasm_module(&kernels, Default::default()).expect("emit the kernel module");
    assert_eq!(flags.len(), 1, "the module reports a SIMD flag for its kernel");
    let scalar = kernel::wasm_module_scalar(&kernels, Default::default()).expect("emit the scalar module");
    for mut bytes in [vectorized, scalar] {
        bytes.resize(bytes.capacity() + 1, 0xff);
        assert!(bytes.starts_with(b"\0asm"), "the wasm magic survives growing the returned buffer");
    }
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
        whole_ranges: false,
        uses: Vec::new(),
        bind: Vec::new(),
    };
    assert!(vm_kernel::compile(&vm, &kernel(&vm), &[good], Backend::Interp, &[]).is_ok());
    let errors = vm_kernel::compile(&vm, &kernel(&vm), &[drifted], Backend::Interp, &[]).err().expect("the drifted table must not compile");
    assert!(errors.iter().any(|e| e.message.contains("`ObjDriftProbe` is not the registered one")), "{errors:?}");
}
