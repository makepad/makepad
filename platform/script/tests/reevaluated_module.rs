//! A module evaluated again with other text (a live edit's reload pass) runs
//! the new text, and a fn made from the earlier text that nothing made again
//! (a type default kept outside the reload, a host's handle) keeps the
//! earlier text's code, in the interpreter and in a shader compile, until
//! nothing holds it. It used to read the new text's opcodes at its old
//! index: a draw shader kept from before an edit failed to compile with
//! "nested function definitions are not supported in shaders".

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

/// The module's text: a draw shader whose `tint` is `level`, with `helpers`
/// fns defined ahead of its own (moving every opcode after them), and a
/// `number` fn returning `n`.
fn edited(helpers: usize, level: &str, n: u32) -> ScriptMod {
    let helpers: String = (0..helpers).map(|k| format!("h{k}: fn() -> vec4 {{ return vec4({k}.0) }}\n")).collect();
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\n{helpers}vertex: fn() {{\nself.vertex_pos = vec4(0.0, 0.0, 0.0, 1.0)\n}}\ntint: fn() -> vec4 {{\nreturn vec4({level}, 0.0, {level}, 1.0)\n}}\nfragment: fn() {{\nself.pixel = self.tint()\n}}\nnumber: fn() {{ return {n} }}\n}}"
    );
    ScriptMod::new("", "", "edited.rs", 12, 5, &code, &[])
}

fn metal(vm: &mut ScriptVm, shader: ScriptObject) -> String {
    let compile = vm
        .eval(ScriptMod::new("", "", "compile.rs", 0, 0, "use mod.shader\nfn(sh) { return shader.test_compile_draw_source(sh, \"metal\", false) }", &[]))
        .as_object()
        .expect("a fn");
    let source = vm.call(compile.into(), &[shader.into()]);
    vm.bx.heap.string_with(source, |_, s| s.to_string()).unwrap_or_else(|| format!("ERRORS: not a string {source:?}"))
}

fn fn_of(vm: &ScriptVm, obj: ScriptObject, name: LiveId) -> (ScriptObject, ScriptIp) {
    let f = vm.bx.heap.value(obj, name.into(), NoTrap).as_object().expect("a fn");
    match vm.bx.heap.as_fn(f) {
        Some(ScriptFnPtr::Script(ip)) => (f, ip),
        _ => panic!("not a script fn"),
    }
}

#[test]
fn a_fn_keeps_its_text_when_its_module_is_evaluated_again_with_other_text() {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let before = vm.eval(edited(0, "0.125", 7)).as_object().expect("the shader");
    let kept = vm.bx.heap.new_object_ref(before);
    let after = vm.eval(edited(12, "0.875", 8)).as_object().expect("the edited shader");
    let _after = vm.bx.heap.new_object_ref(after);

    let edited_src = metal(&mut vm, after);
    assert!(!edited_src.starts_with("ERRORS") && edited_src.contains("0.875"), "premise: the edit compiles\n{edited_src}");
    let kept_src = metal(&mut vm, before);
    assert!(!kept_src.starts_with("ERRORS"), "the kept shader compiles\n{kept_src}");
    assert!(kept_src.contains("0.125") && !kept_src.contains("0.875"), "from its own text\n{kept_src}");

    let (number, ip) = fn_of(&vm, before, id!(number));
    let (edited_number, edited_ip) = fn_of(&vm, after, id!(number));
    assert_eq!(vm.call(number.into(), &[]).as_number(), Some(7.0), "the kept fn runs its own code");
    assert_eq!(vm.call(edited_number.into(), &[]).as_number(), Some(8.0));
    assert_ne!(ip.body, edited_ip.body, "the earlier text is a body of its own");

    // Once nothing holds a fn of it, the earlier text goes: edits do not pile up.
    drop(kept);
    vm.eval(ScriptMod::new("", "", "other.rs", 0, 0, "1", &[]));
    vm.gc();
    assert!(matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free));
    assert!(matches!(&vm.bx.code.bodies.borrow()[edited_ip.body as usize].source, ScriptSource::Mod(m) if m.file == "edited.rs"));
}
