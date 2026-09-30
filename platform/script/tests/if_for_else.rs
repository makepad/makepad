//! Regression tests: an `if` branch whose block holds a `for` loop, followed
//! by `else`.
//!
//! Found by the frontier game lane (2026-09-27): `if c { for … { } } else
//! { … }` fails at eval with "pop_stack_value on empty stack": an arm that
//! ends in a loop, a `let`, a `;` or nothing leaves no value while the other
//! arm leaves one. Pushing nil for such arms fixed scripts but broke shader
//! compilation (shader ifs are statements whose arms must stay valueless).
//! Fixed in the parser (2026-09-29): an if in statement position whose arms
//! all leave no value leaves none; otherwise (one arm valued, or expression
//! position) a valueless arm yields nil through IF_ELSE's NEED_NIL or a
//! trailing NIL_ARM, both of which the shader compiler ignores. A match is
//! an expression: its valueless arms always yield nil.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn run(name: &str, code: &str) -> (ScriptValue, Vec<String>, ScriptVm<'static>) {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("if_for_else_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs, vm)
}

fn number(code: &str, name: &str) -> f64 {
    let (value, errs, _vm) = run(name, code);
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    value.as_number().unwrap_or_else(|| panic!("{name}: not a number: {value:?}"))
}

#[test]
fn if_branch_ending_in_a_for_loop_then_else() {
    let code = "fn f(k) { let n = 0\n if k == 1 { for i in 0..3 { n += 1 } } else { n = 100 }\n return n }\nf(1) * 1000 + f(2)";
    assert_eq!(number(code, "ending"), 3100.0);
}

#[test]
fn if_branch_with_a_for_loop_then_more_then_else() {
    // The frontier repro: the loop body holds its own if/else.
    let code = "fn f(kind) {\n let xs = []\n if kind == \"a\" { for i in 0..3 { if i == 1 { xs.push(i) } else { xs.push(2) } }\n xs.push(9) } else { xs.push(1) }\n return xs\n}\nf(\"a\").len() * 10 + f(\"b\").len()";
    assert_eq!(number(code, "frontier"), 41.0);
}

#[test]
fn every_arm_shape_leaves_one_value() {
    // An arm ending in `;`, a `let`, nothing, or a loop is worth nil; the
    // other arm keeps its value; nothing is leaked or over-popped.
    let w = |body: &str| format!("fn f(k) {{ let n = 0\n {body}\n return n }}\nf(1) * 1000 + f(2)");
    for (name, body, want) in [
        ("sep", "if k == 1 { n = 5; } else { n = 50 }", 5050.0),
        ("sep_else", "if k == 1 { n = 5 } else { n = 50; }", 5050.0),
        ("empty", "if k == 1 { } else { n = 50 }", 50.0),
        ("for_noelse", "if k == 1 { for i in 0..4 { n += 1 } }", 4000.0),
        ("else_for", "if k == 1 { n = 5 } else { for i in 0..4 { n += 1 } }", 5004.0),
        ("let_last", "if k == 1 { n = 3 let z = 3 } else { n = 50 }", 3050.0),
        ("nested", "if k == 1 { if n == 0 { for i in 0..2 { n += 1 } } else { n = 9 } } else { for i in 0..3 { n += 2 } }", 2006.0),
    ] {
        assert_eq!(number(&w(body), name), want, "{name}");
    }
    let value = "fn f(k) { let v = if k == 1 { 3 } else { 4 }\n return v }\nf(1) * 10 + f(2)";
    assert_eq!(number(value, "value"), 34.0);
    let nil_arm = "fn f(k) { let v = if k == 1 { for i in 0..2 { } } else { 4 }\n if v == nil { return 1 } return v }\nf(1) * 10 + f(2)";
    assert_eq!(number(nil_arm, "nil_arm"), 14.0);
}

#[test]
fn top_level_if_for_else() {
    let code = "let n = 0\nlet k = 1\nif k == 1 { for i in 0..4 { n += 1 } } else { n = 50 }\nfn get() { return n }\nget()";
    assert_eq!(number(code, "toplevel"), 4.0);
}

fn nil_or_number(code: &str, name: &str) -> Option<f64> {
    let (value, errs, _vm) = run(name, code);
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    if value.is_nil() {
        return None;
    }
    Some(value.as_number().unwrap_or_else(|| panic!("{name}: not a number: {value:?}")))
}

#[test]
fn statement_ifs_without_else() {
    let w = |body: &str| format!("fn f(k) {{ let n = 0\n {body}\n return n }}\nf(1) * 1000 + f(2)");
    for (name, body, want) in [
        ("sep_noelse", "if k == 1 { n = 5; }", 5000.0),
        ("let_noelse", "if k == 1 { let z = 1 n = 2 let y = 3 }", 2000.0),
        ("empty_noelse", "if k == 1 { }", 0.0),
        ("ret_noelse", "if k == 1 { return 7; }", 7000.0),
        ("ret_value_noelse", "if k == 1 { return 7 }", 7000.0),
        ("two_in_a_row", "if k == 1 { n = 1; } if k == 2 { for i in 0..3 { n += 1 } }", 1003.0),
        ("expr_arm", "if k == 1 n = 4", 4000.0),
    ] {
        assert_eq!(number(&w(body), name), want, "{name}");
    }
}

#[test]
fn else_if_chains_and_nesting() {
    let w = |body: &str| format!("fn f(k) {{ let n = 0\n {body}\n return n }}\nf(1) * 10000 + f(2) * 100 + f(3)");
    for (name, body, want) in [
        ("chain_for_mid", "if k == 1 { n = 1 } else if k == 2 { for i in 0..2 { n += 1 } } else { n = 3 }", 10203.0),
        ("chain_all_valueless", "if k == 1 { n = 1; } else if k == 2 { for i in 0..2 { n += 1 } } else { let z = 0 n = 3; }", 10203.0),
        ("chain_no_final_else", "if k == 1 { for i in 0..5 { n += 1 } } else if k == 2 { n = 2 }", 50200.0),
        ("elif_chain", "if k == 1 { n = 1; } elif k == 2 { n = 2 } else { for i in 0..3 { n += 1 } }", 10203.0),
        ("nested_valueless", "if k != 3 { if k == 1 { for i in 0..4 { n += 1 } } else { n = 2; } } else { n = 3 }", 40203.0),
        ("nested_in_loop", "for j in 0..2 { if k == 1 { for i in 0..2 { n += 1 } } else if k == 2 { n += 5 } else { n += 7; } }", 41014.0),
    ] {
        assert_eq!(number(&w(body), name), want, "{name}");
    }
}

#[test]
fn if_as_the_value_of_a_fn_or_block() {
    // The last expression of a fn body is its value: a valueless arm is nil.
    let mixed = "fn g(k) { if k == 1 { for i in 0..2 { } } else { 5 } }\ng(";
    assert_eq!(nil_or_number(&format!("{mixed}1)"), "last_mixed_1"), None);
    assert_eq!(nil_or_number(&format!("{mixed}2)"), "last_mixed_2"), Some(5.0));
    let valueless = "fn g(k) { let n = 0 if k == 1 { for i in 0..2 { n += 1 } } else { n = 1; } }\ng(1)";
    assert_eq!(nil_or_number(valueless, "last_valueless"), None);
    let noelse = "fn g(k) { if k == 1 { 3 } }\nlet a = g(1) let b = g(2) if b == nil { a } else { 0 }";
    assert_eq!(nil_or_number(noelse, "last_noelse"), Some(3.0));
    // An if as the value of an enclosing arm block.
    let inner = "fn g(k) { let v = if k > 0 { if k == 1 { for i in 0..2 { } } else { k } } else { 4 }\n return v }\n";
    assert_eq!(nil_or_number(&format!("{inner}g(1)"), "inner_1"), None);
    assert_eq!(nil_or_number(&format!("{inner}g(2)"), "inner_2"), Some(2.0));
    assert_eq!(nil_or_number(&format!("{inner}g(0)"), "inner_0"), Some(4.0));
    // Expression position: every valueless shape is nil.
    for (name, expr) in [
        ("expr_noelse", "if k == 1 { for i in 0..2 { } }"),
        ("expr_both", "if k == 1 { for i in 0..2 { } } else { let z = 1 }"),
        ("expr_sep", "if k == 1 { 5; } else { 6; }"),
        ("expr_chain", "if k == 1 { for i in 0..2 { } } else if k == 2 { } else { 3; }"),
    ] {
        for k in 1..=3 {
            let code = format!("fn g(k) {{ let v = {expr}\n return v }}\ng({k})");
            assert_eq!(nil_or_number(&code, name), None, "{name} k={k}");
        }
    }
    // A loop as the last statement of a fn body leaves no value.
    let tail_loop = "fn g() { let n = 0 for i in 0..3 { n += 1 } }\nlet r = g() if r == nil { 1 } else { 0 }";
    assert_eq!(number(tail_loop, "tail_loop"), 1.0);
}

#[test]
fn match_arms_leave_one_value() {
    let w = |body: &str| format!("fn f(k) {{ let n = 0\n {body}\n return n }}\nf(1) * 1000 + f(2)");
    for (name, body, want) in [
        ("match_mixed", "match k { 1 => { for i in 0..4 { n += 1 } } _ => { n = 50 } }", 4050.0),
        ("match_sep", "match k { 1 => { n = 5; } 2 => { n = 50 } }", 5050.0),
        ("match_no_wildcard", "match k { 1 => { for i in 0..4 { n += 1 } } 3 => 9 }", 4000.0),
        ("match_all_valueless", "match k { 1 => { n = 1; } _ => { for i in 0..2 { n += 1 } } }", 1002.0),
    ] {
        assert_eq!(number(&w(body), name), want, "{name}");
    }
    let value = "fn g(k) { let v = match k { 1 => { for i in 0..2 { } } 2 => 5 }\n return v }\n";
    assert_eq!(nil_or_number(&format!("{value}g(1)"), "match_value_1"), None);
    assert_eq!(nil_or_number(&format!("{value}g(2)"), "match_value_2"), Some(5.0));
    assert_eq!(nil_or_number(&format!("{value}g(3)"), "match_value_3"), None);
}

#[test]
fn the_repro_from_the_brief() {
    let code = "fn f(k) { let rows = []; if k == 1 { for a in [1,2] { rows.push(a) } } else { rows.push(2) }; return rows }\nf(1).len() * 10 + f(2).len()";
    assert_eq!(number(code, "brief"), 21.0);
}

/// A closure passed straight into a call, whose body ends in a statement if
/// holding a loop, is still a function (leap's `game.on_touch(|a, b, side|
/// { … if t == "portal" { for p in portals { … } } })`). The body's
/// valueless-end marker used to survive the removal of the body's slot
/// frame placeholder, land on the code right after the body, and make the
/// fn literal read as a statement that leaves no value: the call got nil,
/// the game's touch handler was never registered and its portals did
/// nothing.
#[test]
fn a_closure_argument_ending_in_a_statement_if_with_a_loop_is_a_function() {
    let take = "let seen = []\nfn take(f) { seen.push(f) }\nfn call(f) { return f(3) }\nfn keep(f) { return f }\n";
    for (name, closure) in [
        ("if_for", "|a| { if a > 0 { for s in [1, 2] { let k = s } } }"),
        ("if_for_multiline", "|a| {\n    if a > 0 {\n        for s in [1, 2] {\n            let k = s\n        }\n    }\n}"),
        ("if_for_if", "|a| { let t = 1\n if t == 1 { for s in [1] { if s == a { t = 2 } } } }"),
        ("nested_closure", "|a| { if a > 0 { for s in [1] { keep(|| { let q = s }) } } }"),
        ("if_else_for", "|a| { if a > 0 { for s in [1] { let k = s } } else { for s in [2] { let k = s } } }"),
    ] {
        let code = format!("{take}take({closure})\nlet f = seen[0]\ncall(f)\nseen.len()");
        assert_eq!(number(&code, name), 1.0, "{name}: the closure argument arrived");
        let (value, errs, _vm) = run(name, &format!("{take}call({closure})\n7"));
        assert!(errs.is_empty(), "{name}: calling the closure argument failed: {errs:?}");
        assert_eq!(value.as_number(), Some(7.0), "{name}");
        // Two arguments, the closure last, as `game.after(secs, || { … })`.
        let two = format!("{take}fn later(n, f) {{ return f(n) }}\nlater(3, {closure})\n9");
        assert_eq!(number(&two, name), 9.0, "{name}: second argument");
    }
}

mod shader_ifs {
    //! Shader ifs are statements: their arms stay valueless whatever the
    //! parser does for script ifs. Every if/match shape compiles on every
    //! backend. SHADER_IFS_DUMP=<dir> writes the generated sources: against
    //! the parser before the fix they were identical (bar a match temp's
    //! position-derived name), and it could not compile `tail_if` at all.
    use makepad_script::makepad_math::*;
    use makepad_script::traits::*;
    use makepad_script::*;

    #[derive(Script, ScriptHook)]
    #[repr(C)]
    pub struct ShaderIfsTest {
        #[live]
        pub tint: Vec4f,
    }

    const SHADER: &str = r#"
        use mod.shader
        use mod.pod.*
        use mod.math.*

        let sh = #(0){
            vertex_pos: shader.vertex_position(vec4f)
            fb0: shader.fragment_output(0, vec4f)
            vertex: fn() {
                return vec4(0.0, 0.0, 0.0, 1.0)
            }
            sep_noelse: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { r = 1.0; }
                return r
            }
            value_noelse: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { r = 1.0 }
                return r
            }
            for_else: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { for i in 0..3 { r += 1.0 } } else { r = 5.0 }
                return r
            }
            else_for: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { r = 5.0 } else { for i in 0..3 { r += 1.0 } }
                return r
            }
            let_arms: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { let y = x r = y } else { let z = x * 2.0 r = z; }
                return r
            }
            chain: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.8 { r = 3.0; } else if x > 0.5 { for i in 0..2 { r += 1.0 } } else if x > 0.2 { r = 1.0 } else { let q = 1.0 }
                return r
            }
            nested: fn(x: f32, y: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { if y > 0.5 { r = 1.0; } else { for i in 0..2 { r += 0.5 } } } else { r = 2.0 }
                return r
            }
            ret_sep: fn(x: f32) -> f32 {
                if x > 0.5 { return 1.0; }
                if x > 0.2 { return 0.5; } else { return 0.25; }
            }
            ret_mixed: fn(x: f32) -> f32 {
                var r = 0.0
                if x > 0.5 { return 1.0 } else { r = 2.0; }
                return r
            }
            select: fn(x: f32) -> f32 {
                let v = if x > 0.5 { 1.0 } else { 2.0 }
                return v
            }
            matched: fn(x: f32) -> f32 {
                var r = 0.0
                let k = i32(x * 4.0)
                match k {
                    0 => { r = 1.0; }
                    1 => { for i in 0..2 { r += 1.0 } }
                    _ => { r = 3.0 }
                }
                return r
            }
            tail_if: fn(x: f32) {
                if x > 0.5 { self.fb0 = vec4(1.0, 0.0, 0.0, 1.0); } else { self.fb0 = vec4(0.0); }
            }
            fragment: fn() {
                let x = self.tint.x
                let y = self.tint.y
                let a = self.sep_noelse(x) + self.value_noelse(x) + self.for_else(x) + self.else_for(x)
                let b = self.let_arms(x) + self.chain(x) + self.nested(x, y) + self.ret_sep(x)
                let c = self.ret_mixed(x) + self.select(x) + self.matched(x)
                self.tail_if(x)
                self.fb0 = vec4(a, b, c, 1.0)
            }
        }
    "#;

    #[test]
    fn shader_ifs_compile_on_every_backend() {
        let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
        let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
        let dump = std::env::var("SHADER_IFS_DUMP").ok();
        for backend in ["metal", "hlsl", "glsl", "wgsl"] {
            let shader_obj = ShaderIfsTest::script_shader(&mut vm);
            let value = vm.eval(ScriptMod {
                cargo_manifest_path: env!("CARGO_MANIFEST_DIR").to_string(),
                module_path: "shader_ifs".to_string(),
                file: "shader_ifs.rs".to_string(),
                line: 1,
                column: 1,
                code: format!("{SHADER}\n        shader.test_compile_draw_source(sh, \"{backend}\", false)"),
                values: vec![shader_obj],
            });
            assert!(!value.is_err(), "{backend}: script errored: {value:?}");
            let source = vm.bx.heap.string_with(value, |_heap, s| s.to_string()).unwrap_or_default();
            assert!(!source.is_empty() && !source.starts_with("ERRORS"), "{backend}: {source}");
            assert!(!source.contains("nil"), "{backend}: a nil reached the shader:\n{source}");
            if let Some(dir) = &dump {
                std::fs::write(format!("{dir}/{backend}.txt"), &source).unwrap();
            }
        }
    }
}
