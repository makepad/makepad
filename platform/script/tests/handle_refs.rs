//! Host references are ordinary script handles: a miss in property position is an
//! evaluation error with the object's own keys, and a minted handle survives let,
//! object literals, arrays, call arguments and an object's property.

use makepad_script::*;

struct FieldRef(u32);
impl ScriptHandleGc for FieldRef {
    fn gc(&mut self) {}
    fn debug_fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "field derby.{}", self.0)
    }
}

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    vm
}

/// A frozen api object `race` with fields lap, laps, rank (handles), and a plain
/// object `plain` with the same keys.
fn install(vm: &mut ScriptVm) {
    let ht = vm.new_handle_type(id!(field));
    for (name, frozen) in [(id_lut!(race), true), (id_lut!(plain), false)] {
        let m = vm.new_module(name);
        for (i, key) in [id_lut!(lap), id_lut!(laps), id_lut!(rank)].into_iter().enumerate() {
            let h = vm.bx.heap.new_handle(ht, Box::new(FieldRef(i as u32)));
            vm.bx.heap.set_value_def(m, key.into(), h.into());
        }
        if frozen {
            vm.bx.heap.freeze_api(m);
        }
    }
}

fn run(vm: &mut ScriptVm, name: &str, code: &str) -> (ScriptValue, Vec<String>) {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", &format!("{name}.splash"), 0, 0, code, &[]));
    (v, vm.take_errors())
}

fn errors_of(vm: &mut ScriptVm, name: &str, code: &str) -> Vec<String> {
    run(vm, name, code).1
}

#[test]
fn a_miss_in_property_position_is_an_evaluation_error_with_its_place() {
    let mut vm = test_vm();
    install(&mut vm);
    // Bare, in an object literal property value, in a call argument.
    for (name, code, line_col) in [
        ("direct", "mod.race.lpa", "direct.splash:0:9:"),
        ("lit", "let w = {bind: mod.race.lpa}\nw", "lit.splash:0:24:"),
        ("multi", "let a = 1\nlet w = {\n  a: a\n  bind: mod.race.lpa\n}\nw", "multi.splash:3:17:"),
        ("arg", "fn f(x) { x }\nf(mod.race.lpa)", "arg.splash:1:11:"),
    ] {
        let errors = errors_of(&mut vm, name, code);
        assert_eq!(errors.len(), 1, "{name}: {errors:?}");
        assert!(errors[0].starts_with(line_col), "{name}: {errors:?}");
        // A frozen API object lists its own keys, not a guess.
        assert!(errors[0].contains("no field `lpa`; it has lap, laps, rank"), "{name}: {errors:?}");
    }
    // A hit raises nothing.
    assert!(errors_of(&mut vm, "hit", "let w = {bind: mod.race.lap}\nw").is_empty());
}

#[test]
fn an_ordinary_object_keeps_its_did_you_mean_message() {
    let mut vm = test_vm();
    install(&mut vm);
    let errors = errors_of(&mut vm, "plain", "let w = {bind: mod.plain.lpa}\nw");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("property lpa not found in prototype chain. Did you mean: lap"), "{errors:?}");
    assert!(!errors[0].contains("no field"), "{errors:?}");
}

#[test]
fn a_host_handle_passes_through_every_position_and_prints() {
    let mut vm = test_vm();
    install(&mut vm);
    let lap = {
        let m = vm.modules();
        let race = vm.value(m, id_lut!(race)).as_object().unwrap();
        vm.value(race, id_lut!(lap))
    };
    assert!(lap.is_handle(), "{lap:?}");
    let code = "let h = mod.race.lap
fn pass(x) { x }
let w = {bind: h, list: [h, mod.race.lap]}
let again = pass(w.bind)
let same = again == mod.race.lap && w.list[0] == h && w.list[1] == h && h != mod.race.laps && w.bind == h
{same: same, h: h, w: w, via_fn: again}";
    let (v, errors) = run(&mut vm, "round", code);
    assert!(errors.is_empty(), "{errors:?}");
    let out = v.as_object().unwrap();
    let get = |vm: &ScriptVm, k: LiveId| vm.value(out, k);
    assert_eq!(get(&vm, id_lut!(same)).as_bool(), Some(true));
    assert_eq!(get(&vm, id_lut!(h)), lap);
    assert_eq!(get(&vm, id_lut!(via_fn)), lap);
    // The host reads its payload back, and `log` prints the host's text.
    let h = get(&vm, id_lut!(h)).as_handle().unwrap();
    assert_eq!(vm.bx.heap.handle_ref::<FieldRef>(h).map(|f| f.0), Some(0));
    let mut text = String::new();
    let mut seen = Vec::new();
    vm.bx.heap.to_debug_string(get(&vm, id_lut!(w)), &mut seen, &mut text, true, 0);
    assert!(text.contains("field derby.0"), "{text}");
}
