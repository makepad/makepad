//! Regression tests: a source that ends without a trailing newline.
//!
//! A number, identifier, operator or color is only emitted by the character
//! after it, so a source ending in one lost its last token: `x * 100` at
//! EOF parsed as `x *` and failed with "pop_stack_resolved on empty stack".
//! Common in AI-written sources. Full evals now flush the tokenizer at the
//! end (ScriptTokenizer::finish); streamed evals parse the pending token
//! provisionally and re-lex it when more source arrives (pending_token).

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn script_mod(file: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: file.to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

fn errors(vm: &mut ScriptVm) -> Vec<String> {
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    errs
}

/// Every ending, as `(name, source, expected number)`; nil is -1.
const ENDINGS: &[(&str, &str, f64)] = &[
    ("number", "let x = 2\nx * 100", 200.0),
    ("float", "let x = 2\nx * 1.5", 3.0),
    ("bare_number", "42", 42.0),
    ("string", "let s = \"abc\"\ns.len() + \"de\".len()", 5.0),
    ("string_last", "let n = 1\n\"xyz\"", -2.0),
    ("ident", "let answer = 7\nanswer", 7.0),
    ("call", "fn f(a) { return a * 3 }\nf(4)", 12.0),
    ("closing_brace", "fn f() { return 9 }\nlet v = f()\nif v == 9 { 1 } else { 0 }", 1.0),
    ("atom", "let a = @d3\nif a == @d3 { 1 } else { 0 }\n@d3", -3.0),
    ("line_comment", "let x = 5\nx * 2 // done", 10.0),
    ("block_comment", "let x = 5\nx * 3 /* done */", 15.0),
    ("unclosed_comment", "let x = 5\nx * 4 /* never closed", 20.0),
    ("color", "let c = #f00\nc", -4.0),
    ("negative", "let x = 3\nx - 10", -7.0),
];

fn check(name: &str, value: ScriptValue, want: f64, errs: Vec<String>) {
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    match want {
        // A string, an atom and a color: just not nil.
        w if w == -2.0 || w == -3.0 || w == -4.0 => {
            assert!(!value.is_nil(), "{name}: the last value was lost: {value:?}")
        }
        _ => assert_eq!(value.as_number(), Some(want), "{name}: {value:?}"),
    }
}

#[test]
fn full_eval_keeps_the_last_token() {
    for &(name, code, want) in ENDINGS {
        let mut vm = test_vm();
        vm.bx.captured_errors = Some(Vec::new());
        let value = vm.with_instruction_limit(100_000, |vm| vm.eval(script_mod(&format!("eof_{name}"), code)));
        let errs = errors(&mut vm);
        check(name, value, want, errs);
    }
}

#[test]
fn streamed_eval_keeps_the_last_token() {
    for &(name, code, want) in ENDINGS {
        // The whole source in one append, then char by char: every prefix
        // runs, and the final one must equal the full eval.
        let mut vm = test_vm();
        vm.bx.captured_errors = Some(Vec::new());
        let value = vm.with_instruction_limit(100_000, |vm| {
            vm.eval_with_append_source(script_mod(&format!("eof_one_{name}"), ""), code, NIL.into())
        });
        let errs = errors(&mut vm);
        check(name, value, want, errs);

        let mut vm = test_vm();
        let mut value = NIL;
        for end in (1..=code.len()).filter(|end| code.is_char_boundary(*end)) {
            value = vm.with_instruction_limit(100_000, |vm| {
                vm.eval_with_append_source(script_mod(&format!("eof_chars_{name}"), ""), &code[..end], NIL.into())
            });
        }
        vm.bx.captured_errors = Some(Vec::new());
        check(&format!("{name} (char by char)"), value, want, errors(&mut vm));
    }
}

#[test]
fn a_streamed_number_is_re_lexed_when_it_grows() {
    let mut vm = test_vm();
    let mut value = NIL;
    for code in ["let x = 1", "let x = 12", "let x = 123\nx + 1", "let x = 123\nx + 10"] {
        value = vm.with_instruction_limit(100_000, |vm| {
            vm.eval_with_append_source(script_mod("eof_grow", ""), code, NIL.into())
        });
    }
    assert_eq!(value.as_number(), Some(133.0));
}

/// An if still open when the streamed source stops (`if c { 1 } else`) left
/// a zero jump offset: the prefix looped until the instruction limit.
#[test]
fn no_streamed_prefix_loops_forever() {
    for &(name, code, _want) in ENDINGS {
        let mut vm = test_vm();
        for end in (1..=code.len()).filter(|end| code.is_char_boundary(*end)) {
            vm.bx.captured_errors = Some(Vec::new());
            vm.with_instruction_limit(100_000, |vm| {
                vm.eval_with_append_source(script_mod(&format!("zz_{name}"), ""), &code[..end], NIL.into())
            });
            let errs = errors(&mut vm);
            assert!(
                !errs.iter().any(|e| e.contains("instruction limit")),
                "{name}: prefix {:?} ran into the instruction limit",
                &code[..end]
            );
        }
    }
}

/// A field or index assignment as the source's last statement is stored
/// (it was dropped: the end of source closed the pending assignment
/// without emitting it).
#[test]
fn a_last_field_or_index_assignment_is_stored() {
    for (name, code) in [
        ("field", "let o = {a: 1}\no.b = 5\nnil\nlet p = o\np.c = 7"),
        ("index", "let o = {a: 1}\nlet arr = [0, 0]\no.arr = arr\narr[1] = 7"),
        ("field_newline", "let o = {a: 1}\no.c = 7\n"),
    ] {
        let mut vm = test_vm();
        vm.bx.captured_errors = Some(Vec::new());
        let code = format!("let keep = {{}}\nmod.std.keep = keep\n{}", code.replace("let o = {a: 1}", "let o = keep"));
        vm.eval(script_mod(name, &code));
        assert!(errors(&mut vm).is_empty(), "{name}");
        let std = vm.bx.heap.value(vm.bx.heap.modules, id!(std).into(), NoTrap).as_object().unwrap();
        let o = vm.bx.heap.value(std, id!(keep).into(), NoTrap).as_object().unwrap();
        let got = if name == "index" {
            let a = vm.bx.heap.value(o, id!(arr).into(), NoTrap).as_array().unwrap();
            vm.bx.heap.array_index(a, 1, NoTrap).as_number()
        } else {
            vm.bx.heap.value(o, id!(c).into(), NoTrap).as_number()
        };
        assert_eq!(got, Some(7.0), "{name}");
    }
}
