//! Regression tests: nested array literals with space-separated rows.
//!
//! Found by Stage's score lane (2026-09-29): `[[@d3 @a3] [@c3 @g3]]` failed
//! with "Expected ]" at the second row's `@`: the `[` of the second row was
//! parsed as an index into the first (`[@d3 @a3][@c3 @g3]`). Array literal
//! elements may be space separated, so inside one a `[` after whitespace now
//! starts the next element; `a[i]` (no space) still indexes. Shaders have no
//! array literals, so they are unaffected.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn number(name: &str, code: &str) -> f64 {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("nested_arrays_{name}"),
            line: 0,
            column: 0,
            // A number ending the source without a newline is still in the
            // tokenizer when eval parses (a separate, pre-existing issue).
            code: format!("{code}\n"),
            values: vec![],
        })
    });
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    value.as_number().unwrap_or_else(|| panic!("{name}: not a number: {value:?}"))
}

/// The shape of a two-row literal: rows * 100 + row lengths, [[a b] [c d e]] -> 223.
fn shape(name: &str, literal: &str) -> f64 {
    number(name, &format!("let a = {literal}\na.len() * 100 + a[0].len() * 10 + a[a.len() - 1].len()"))
}

#[test]
fn rows_of_atoms() {
    for (name, literal) in [
        ("space", "[[@d3 @a3] [@c3 @g3 @e3]]"),
        ("comma", "[[@d3, @a3], [@c3, @g3, @e3]]"),
        ("inner_space_outer_comma", "[[@d3 @a3], [@c3 @g3 @e3]]"),
        ("inner_comma_outer_space", "[[@d3, @a3] [@c3, @g3, @e3]]"),
        ("newlines", "[\n [@d3 @a3]\n [@c3 @g3 @e3]\n]"),
    ] {
        assert_eq!(shape(name, literal), 223.0, "{name}");
    }
    let values = "let a = [[@d3 @a3] [@c3 @g3]]\nif a[1][0] == @c3 && a[0][1] == @a3 { 1 } else { 0 }";
    assert_eq!(number("values", values), 1.0);
    // An atom first in every row, one element per row.
    assert_eq!(shape("single", "[[@d3] [@c3]]"), 211.0);
    // No space: the second `[` still indexes the first row.
    assert_eq!(number("tight", "let a = [[@d3 @a3][1]]\nif a[0] == @a3 { 1 } else { 0 }"), 1.0);
}

#[test]
fn rows_of_numbers_and_mixed() {
    for (name, literal) in [
        ("numbers_space", "[[1 2] [3 4 5]]"),
        ("numbers_comma", "[[1, 2], [3, 4, 5]]"),
        ("mixed", "[[@d3 2] [3 @g3 \"x\"]]"),
        ("mixed_first_number", "[[1 @a3] [@c3 4 5]]"),
        ("negative", "[[-1 2] [3, -4, 5]]"),
    ] {
        assert_eq!(shape(name, literal), 223.0, "{name}");
    }
    // Three levels deep.
    let deep = "let a = [[[1 2] [3]] [[4] [5 6 7]]]\na.len() * 100 + a[1].len() * 10 + a[1][1].len()";
    assert_eq!(number("deep", deep), 223.0);
    let sum = "let a = [[1 2] [3 4]]\na[0][0] + a[0][1] * 10 + a[1][0] * 100 + a[1][1] * 1000";
    assert_eq!(number("sum", sum), 4321.0);
}

#[test]
fn indexing_still_indexes() {
    // No space before `[`: an index, inside and outside array literals.
    let code = "let a = [[1 2] [3 4]]\nlet b = [a[1][0] a[0][1]]\nb[0] * 10 + b[1]";
    assert_eq!(number("index", code), 32.0);
    let call = "fn f(rows) { return rows.len() * 10 + rows[1].len() }\nf([[@a @b] [@c]])";
    assert_eq!(number("call_arg", call), 21.0);
    // In parentheses inside an array literal, `[` after a space indexes.
    let paren = "let a = [5 6]\nlet b = [(a [1])]\nb[0]";
    assert_eq!(number("paren", paren), 6.0);
}


