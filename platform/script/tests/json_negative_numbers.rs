//! Regression: JSON negative numbers must survive `parse_json`.
//!
//! Empirically (host_launcher, 2026-08-14): the tokenizer emits a leading `-`
//! as its own Operator token, and the value positions had no case for it — so
//! the sign was swallowed AND, inside an object, the key it belonged to was
//! dropped entirely. `{"lat":37.7,"lon":-122.4}` parsed to `{"lat":37.7}`,
//! silently. A mini-app asking the host for a location got coordinates with
//! no longitude and quietly fell back to a default city; sub-zero
//! temperatures and negative UTC offsets had the same fate.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm {
        host,
        std,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval_str(vm: &mut ScriptVm, code: &str) -> String {
    let v = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "json_negative".to_string(),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let mut out = String::new();
    vm.string_with(v, |_vm, s| out = s.to_string());
    out
}

#[test]
fn object_keeps_negative_values_and_their_keys() {
    let mut vm = test_vm();
    let out = eval_str(
        &mut vm,
        "let o = \"{\\\"lat\\\":37.7,\\\"lon\\\":-122.4,\\\"n\\\":-5}\".parse_json()\n(\"\" + o.to_json())",
    );
    assert!(out.contains("\"lon\":-122.4"), "lon lost: {out}");
    assert!(out.contains("\"n\":-5"), "negative int lost: {out}");
    assert!(out.contains("\"lat\":37.7"), "positive lost: {out}");
}

#[test]
fn arrays_and_nested_values_keep_the_sign() {
    let mut vm = test_vm();
    let arr = eval_str(&mut vm, "let a = \"[-1, 2, -3.5]\".parse_json()\n(\"\" + a.to_json())");
    assert_eq!(arr, "[-1,2,-3.5]");
    // Nested, which is the shape real payloads arrive in (a forecast row, a
    // timezone offset). A bare scalar root like `"-42"` stays unsupported —
    // `"42"` never parsed either, so that is a separate, pre-existing gap.
    let nested = eval_str(
        &mut vm,
        "let o = \"{\\\"d\\\":{\\\"lo\\\":-7},\\\"z\\\":[-14400]}\".parse_json()\n(\"\" + o.to_json())",
    );
    assert!(nested.contains("\"lo\":-7"), "nested object negative lost: {nested}");
    assert!(nested.contains("[-14400]"), "nested array negative lost: {nested}");
}
