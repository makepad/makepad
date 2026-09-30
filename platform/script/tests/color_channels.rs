//! A colour's channels read as fields in scripts, 0..1 as written (the
//! `.r .g .b .a` a shader reads).

use makepad_script::*;

#[test]
fn a_colour_has_its_channels_as_fields() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    let mut eval = |code: &str| vm.eval(ScriptMod { file: "color".into(), code: code.into(), ..Default::default() }).as_number();
    assert_eq!(eval("let c = #ff8040\nc.r + c.g * 10 + c.b * 100 + c.a * 1000"), Some(1.0 + 128.0 / 255.0 * 10.0 + 64.0 / 255.0 * 100.0 + 1000.0));
    assert_eq!(eval("#eee9df80.a"), Some(128.0 / 255.0));
    let mut vm2 = ScriptVm { host: Box::leak(Box::new(ScriptVmHost::new(0i32, ()))), bx: Box::new(ScriptVmBase::new()) };
    vm2.bx.captured_errors = Some(Vec::new());
    vm2.eval(ScriptMod { file: "color".into(), code: "#ff8040.x".into(), ..Default::default() });
    let errors = vm2.take_errors();
    assert!(errors.iter().any(|e| e.contains("fields r, g, b and a")), "{errors:?}");
}
