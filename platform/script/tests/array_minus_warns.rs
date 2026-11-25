//! Inside an array literal `[0 0 -30]` is two items, the second `0 - 30`: a `-` after a value is
//! a subtraction, whatever the spaces. The parser warns where that likely reads otherwise to its
//! writer (whitespace before the `-`, none after); commas make the item: `[0, 0, -30]`.
//! Likewise, `..v ..v` spreads one range wherever spreads are accepted.

use makepad_script::syntax::{self, Kind};
use makepad_script::*;

fn eval(code: &str) -> (ScriptVm<'static>, ScriptValue, Vec<String>) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let prelude = "let a = 10\nlet x = 4\nlet f = fn(v) { return v }\nlet v = [5, 6]\nlet o = {b: 3}\n";
    let source = format!("{prelude}{code}");
    let value = vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "array_minus_warns", 0, 0, &source, &[])));
    let warnings = vm.take_warnings();
    let parsed: Vec<_> = makepad_script::parser::parse_warnings(code).into_iter().map(|w| {
        format!("array_minus_warns:{}:{}: {}", prelude.lines().count() + w.line as usize + 1, w.column + 1, w.message)
    }).collect();
    assert_eq!(warnings, parsed, "{code}: evaluated and standalone warnings must agree on location and message");
    (vm, value, warnings)
}

/// The array `code` evaluates to, and the parser's warnings on it (no errors allowed).
fn run(code: &str) -> (Vec<f64>, Vec<String>) {
    let (mut vm, value, warnings) = eval(code);
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{code}: {errors:?}");
    let list = value.as_array().unwrap_or_else(|| panic!("{code}: not a list: {value:?}"));
    let heap = vm.bx.heap_ref();
    ((0..heap.array_len(list)).map(|i| heap.array_index(list, i, NoTrap).as_number().unwrap()).collect(), warnings)
}

/// The syntax tree's item count, which must be the VM's.
fn items(code: &str) -> usize {
    let tree = syntax::parse(code).unwrap();
    let Kind::Array { items, .. } = &tree.statements()[0].kind else { panic!("{code}: not an array") };
    items.len()
}

fn check(code: &str, want: &[f64], warns: bool) {
    let (got, warnings) = run(code);
    assert_eq!(got.len(), want.len(), "{code}: {got:?}");
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-9, "{code}: {got:?}, not {want:?}");
    }
    assert_eq!(items(code), want.len(), "{code}: the syntax tree reads another count");
    assert_eq!(!warnings.is_empty(), warns, "{code}: {warnings:?}");
}

#[test]
fn a_minus_after_a_value_subtracts_inside_brackets() {
    check("[0 0 -30]", &[0.0, -30.0], true);
    check("[1 -2]", &[-1.0], true);
    check("[-1 -2]", &[-3.0], true);
    check("[1mm -2mm 3mm]", &[-0.001, 0.003], true);
    check("[1\n-2]", &[-1.0], true);
}

#[test]
fn the_warning_says_what_it_reads_as_with_the_text_written() {
    let (_, warnings) = run("[0 0 -30]");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].ends_with("`0 -30` inside [ ] reads as one value, 0 - 30: write `0 - 30`, or `0, -30` for two items (the comma makes `-30` its own item)"),
        "the warning must explain both readings and name the negative item: {warnings:?}"
    );
    // At the minus, placed as the parser places its errors: line 6 (five lets before it).
    assert!(warnings[0].starts_with("array_minus_warns:6:"), "{warnings:?}");
    let (_, warnings) = run("[a -30]");
    assert!(warnings[0].contains("`a -30` inside [ ] reads as one value, a - 30"), "{warnings:?}");
    let (_, warnings) = run("[1mm -x]");
    assert!(warnings[0].contains("`1mm -x` inside [ ] reads as one value, 1mm - x"), "{warnings:?}");
    // The operands whole, as written: a call, an index, a glued product.
    let (_, warnings) = run("[f(1) -v[0]]");
    assert!(warnings[0].contains("`f(1) -v[0]`"), "{warnings:?}");
    let (_, warnings) = run("[2*3 -1]");
    assert!(warnings[0].contains("`2*3 -1`"), "{warnings:?}");
    let (_, warnings) = run("[0, 1 -o.b]");
    assert!(warnings[0].contains("`1 -o.b`"), "{warnings:?}");

    let nested = "let shelled = m.brep.fillet(shelled, 0.8mm, {edges: [{near: [0mm 40mm 25mm]}, {near: [0mm 40mm -25mm]}, {near: [-40mm 40mm 0mm]}]})";
    let warnings = makepad_script::parser::parse_warnings(nested);
    let [warning] = warnings.as_slice() else { panic!("only the ambiguous minus in a nested selector should warn: {warnings:?}") };
    assert_eq!((warning.line, warning.column), (0, nested.find("-25mm").unwrap() as u32), "the warning points to the minus in the second selector");
    assert!(warning.message.contains("`40mm, -25mm`") && warning.message.contains("`-25mm` its own item"), "the nested warning must show the coordinate edit: {warning:?}");

    let (_, warnings) = run("[f(\n1) -2]");
    assert_eq!(warnings[0].lines().count(), 1, "a multiline operand must still produce a one-line warning");
}

#[test]
fn no_warning_where_the_minus_reads_as_written() {
    check("[a - 30]", &[-20.0], false);
    check("[a-30]", &[-20.0], false);
    check("[x-1 2]", &[3.0, 2.0], false);
    check("[-30 0]", &[-30.0, 0.0], false);
    check("[0, -30]", &[0.0, -30.0], false);
    check("[0, 0, -30]", &[0.0, 0.0, -30.0], false);
    check("[a, -30]", &[10.0, -30.0], false);
    // Call arguments, parentheses and an index are no array literal.
    check("[f(1 -2)]", &[-1.0], false);
    check("[(5 -2)]", &[3.0], false);
    check("[v[1 -1]]", &[5.0], false);
}

#[test]
fn a_warning_never_stops_the_script() {
    let (got, warnings) = run("[0 0 -30deg]");
    assert_eq!(got.len(), 2);
    assert_eq!(warnings.len(), 1);
}

#[test]
fn a_second_spread_without_a_comma_reads_as_a_range() {
    for (code, name, column) in [("[..v ..v]", "v", 6), ("{..o ..o}", "o", 6), ("f(..v ..v)", "v", 7)] {
        let (mut vm, _, warnings) = eval(code);
        let expected = format!("array_minus_warns:6:{column}: `..{name} ..{name}` reads as one spread of the range {name}..{name}: add a comma: `..{name}, ..{name}`");
        assert_eq!(warnings, [expected], "{code}: warn once at the binary operator and show the comma edit");
        let errors = vm.take_errors();
        assert!(errors.iter().any(|e| e.contains("range must start with a number")), "{code}: evaluating the range must still reject its nonnumeric start: {errors:?}");
    }

    let (_, warnings) = run("[..o.b\n..v[0]]");
    assert_eq!(warnings, ["array_minus_warns:7:1: `..o.b ..v[0]` reads as one spread of the range o.b..v[0]: add a comma: `..o.b, ..v[0]`"],
        "the warning must quote both field and index expressions and point to the operator on the second line");

    let (got, warnings) = run("[..v, ..v]");
    assert_eq!(got, [5.0, 6.0, 5.0, 6.0], "the comma must spread both lists");
    assert!(warnings.is_empty(), "comma-separated spreads must be quiet: {warnings:?}");
    let (mut vm, _, warnings) = eval("[[a..x], [a .. x], [a ..x], [..a..x], [..a .. x], [..(a..x)], [..(a ..x)]]");
    assert!(warnings.is_empty(), "ordinary ranges and explicit range spreads must be quiet: {warnings:?}");
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "the numeric range examples must still evaluate: {errors:?}");
}
