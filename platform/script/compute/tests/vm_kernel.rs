//! Kernels made from Splash values (`vm_kernel`): the entry is the VM's fn
//! object and what it reaches is found through the VM's scopes, not by
//! reading document text.
#![cfg(feature = "vm")]

use makepad_script::*;
use makepad_script_compute::kernel::MathMode;
use makepad_script_compute::vm_kernel::{self, Decl, Entry, Record, VmKernel};
use makepad_script_compute::Backend;

fn doc(code: &str) -> (ScriptVm<'static>, ScriptObject) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let v = vm.eval(ScriptMod { file: "doc.splash".into(), code: code.into(), ..Default::default() });
    assert!(vm.take_errors().is_empty());
    let obj = v.as_object().expect("the document's kernel object");
    let _ = vm.bx.heap.new_object_ref(obj);
    (vm, obj)
}

fn field(vm: &ScriptVm, obj: ScriptObject, name: LiveId) -> ScriptObject {
    vm.bx.heap.value(obj, name.into(), NoTrap).as_object().expect("a fn field")
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
    };
    let (compiled, source) = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    assert!(source.text.contains("fn bump") && source.text.contains("fn scale") && source.text.contains("let R = 3.5"), "{}", source.text);
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
    };
    let errors = vm_kernel::compile(&vm, &kernel, &[], Backend::Interp, &[]).err().expect("an error");
    assert!(errors.iter().any(|e| e.message.contains("doc.splash:2:")), "{errors:?}");
}

#[test]
fn a_kernel_object_reads_back_from_the_vm() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let layout = makepad_script_compute::kernel::Layout {
        name: "Dot".into(),
        stride: 4,
        fields: vec![
            makepad_script_compute::kernel::LayoutField { name: "pos".into(), ty: makepad_script_compute::kernel::FieldTy::Vec3, offset: 0 },
            makepad_script_compute::kernel::LayoutField { name: "size".into(), ty: makepad_script_compute::kernel::FieldTy::F32, offset: 3 },
        ],
    };
    vm_kernel::script_mod(&mut vm, std::slice::from_ref(&layout));
    let v = vm.eval(ScriptMod {
        file: "doc.splash".into(),
        code: "use mod.pod.*\nuse mod.kernel.*\nlet GAP = 2.0\n{count: 3 dots: output(Dot) h: input(f32) amp: param(0.5, 0.0, 1.0) math: \"portable\" element: fn(i) { dots[i].pos = vec3(float(i) * GAP, h[i], 0.0) dots[i].size = amp }}".into(),
        ..Default::default()
    });
    assert!(vm.take_errors().is_empty());
    let obj = v.as_object().unwrap();
    let _keep = vm.bx.heap.new_object_ref(obj);
    let k = vm_kernel::from_object(&vm, obj).unwrap();
    assert_eq!(k.entry, Entry::Element);
    assert_eq!(k.math, MathMode::Portable);
    assert_eq!(k.decls.len(), 3);
    let (compiled, _) = vm_kernel::compile(&vm, &k, &[layout], Backend::Interp, &[]).unwrap_or_else(|e| panic!("{e:?}"));
    let h = [10.0f32, 20.0, 30.0];
    let mut dots = vec![0f32; 12];
    let mut call = compiled.call();
    call.input("h", &h).unwrap();
    call.output("dots", &mut dots).unwrap();
    call.run(3).unwrap();
    drop(call);
    assert_eq!(dots, vec![0.0, 10.0, 0.0, 0.5, 2.0, 20.0, 0.0, 0.5, 4.0, 30.0, 0.0, 0.5]);
}
