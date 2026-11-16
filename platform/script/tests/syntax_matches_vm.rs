//! The syntax tree agrees with the VM on list elements and operator grouping.

use makepad_script::syntax::{self, Kind};
use makepad_script::*;

fn vm_len(code: &str) -> f64 {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let source = format!("let a = 0\nlet f = fn(x) {{ return x }}\nlet v = {code}\nv.len()\n;");
    let value = vm.eval(ScriptMod::new("", "", "t", 0, 0, &source, &[]));
    assert!(vm.take_errors().is_empty(), "{code}");
    value.as_number().unwrap_or(-1.0)
}

fn tree_len(code: &str) -> usize {
    let tree = syntax::parse(code).unwrap();
    let Kind::Array { items, .. } = &tree.statements()[0].kind else { panic!("{code}: not an array") };
    items.len()
}

#[test]
fn list_elements_are_counted_alike() {
    for code in [
        "[1 -2]",
        "[1, -2]",
        "[1 - 2, 3]",
        "[-1 -2]",
        "[[1 2] [3 4]]",
        "[a [1]]",
        "[1\n-2\n]",
        "[1 +2]",
        "[1 2 3]",
        "[f(1) f(2)]",
        "[1, 2,\n 3]",
        "[{a: 1} {a: 2}]",
        "[\"a\" \"b\"]",
        "[a.b a]",
    ] {
        let tree = tree_len(code);
        // (`a.b` on a number is not evaluable: only the shapes are compared for it.)
        if code.contains("a.b") {
            assert_eq!(tree, 2, "{code}");
            continue;
        }
        assert_eq!(tree as f64, vm_len(code), "{code}");
    }
}

#[test]
fn operator_chains_group_as_the_vm_evaluates() {
    use syntax::{Grouped, Node, operator_order};

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Value {
        Number(f64),
        Bool(bool),
        Nil,
    }

    impl Value {
        fn number(self, code: &str) -> f64 {
            match self {
                Self::Number(value) => value,
                _ => panic!("{code}: expected a number, got {self:?}"),
            }
        }

        fn truthy(self) -> bool {
            match self {
                Self::Number(value) => value != 0.0,
                Self::Bool(value) => value,
                Self::Nil => false,
            }
        }
    }

    fn evaluate(grouped: &Grouped<'_>, code: &str) -> Value {
        match grouped {
            Grouped::Operand(node) => match &node.kind {
                Kind::Number(value) => Value::Number(*value),
                Kind::Ident(name) => match name.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "nil" => Value::Nil,
                    _ => panic!("{code}: unexpected identifier {name}"),
                },
                Kind::Unary { op, operand } => {
                    let value = evaluate(&operand.grouped(), code);
                    match op.as_str() {
                        "-" => Value::Number(-value.number(code)),
                        "!" => Value::Bool(!value.truthy()),
                        _ => panic!("{code}: unexpected unary operator {op}"),
                    }
                }
                Kind::Paren { items, .. } => {
                    assert_eq!(items.len(), 1, "{code}: parenthesised operand");
                    evaluate(&items[0].grouped(), code)
                }
                _ => panic!("{code}: unexpected operand {node:?}"),
            },
            Grouped::Binary {
                op,
                op_span,
                lhs,
                rhs,
            } => {
                assert_eq!(&code[op_span.clone()], *op, "{code}: operator span");
                assert!(lhs.span().end <= op_span.start, "{code}: left operand span");
                assert!(
                    op_span.end <= rhs.span().start,
                    "{code}: right operand span"
                );
                let left = evaluate(lhs, code);
                match *op {
                    "&&" | "and" => {
                        return if left.truthy() {
                            evaluate(rhs, code)
                        } else {
                            left
                        };
                    }
                    "||" | "or" => {
                        return if left.truthy() {
                            left
                        } else {
                            evaluate(rhs, code)
                        };
                    }
                    "|?" | "??" => {
                        return if left == Value::Nil {
                            evaluate(rhs, code)
                        } else {
                            left
                        };
                    }
                    _ => {}
                }
                let right = evaluate(rhs, code);
                match *op {
                    "==" | "===" => return Value::Bool(left == right),
                    "!=" | "!==" => return Value::Bool(left != right),
                    _ => {}
                }
                let (left, right) = (left.number(code), right.number(code));
                match *op {
                    "+" => Value::Number(left + right),
                    "-" => Value::Number(left - right),
                    "*" => Value::Number(left * right),
                    "/" => Value::Number(left / right),
                    "%" => Value::Number(left % right),
                    "<" => Value::Bool(left < right),
                    ">" => Value::Bool(left > right),
                    "<=" => Value::Bool(left <= right),
                    ">=" => Value::Bool(left >= right),
                    "<<" => Value::Number(((left as u64) << (right as u64)) as f64),
                    ">>" => Value::Number(((left as u64) >> (right as u64)) as f64),
                    "&" => Value::Number(((left as u64) & (right as u64)) as f64),
                    "|" => Value::Number(((left as u64) | (right as u64)) as f64),
                    "^" => Value::Number(((left as u64) ^ (right as u64)) as f64),
                    _ => panic!("{code}: unexpected binary operator {op}"),
                }
            }
        }
    }

    assert!(
        operator_order("*").unwrap() < operator_order("+").unwrap(),
        "* versus +"
    );
    assert_eq!(operator_order("="), None, "=");
    assert_eq!(operator_order("+"), operator_order("-"), "+ versus -");
    assert_eq!(operator_order("and"), operator_order("&&"), "and versus &&");
    assert_eq!(operator_order("or"), operator_order("||"), "or versus ||");
    for op in ["++", "..", "is"] {
        assert!(operator_order(op).is_some(), "{op}");
    }

    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    for (index, code) in [
        "1 - 2 * 3",
        "2 * 3 - 4 / 2",
        "10 - 4 - 3",
        "16 / 4 / 2",
        "7 % 4 * 2",
        "2 + 3 * 4 - 5",
        "1 + 2 < 4",
        "3 < 2 == false",
        "1 + 1 == 2 && 2 * 2 == 4",
        "true || false && false",
        "false && true || true",
        "-2 * 3 + 1",
        "(1 + 2) * 3",
        "2 - -1",
        "1 << 2 + 1",
        "6 & 3 | 8",
        "5 ^ 1 & 3",
        "12 / 2 * 3",
        "8 - 3 + 2",
        "8 / (2 * 2)",
        "3 > 2 == true",
        "2 <= 1 != true",
        "3 >= 2 === true",
        "3 !== 2 == true",
        "!false && 1 < 2",
        "32 >> 1 + 2",
        "1 << 2 << 1",
        "1 | 2 ^ 3 & 4",
        "true or false and false",
        "false and true or true",
        "nil ?? 1 + 2 * 3",
        "nil |? false || true",
        "false ?? true || true",
        "false |? true || true",
        "true || 1 .. 3",
        "false && 1 .. 3",
        "false ?? 1 .. 3",
        "false |? 1 .. 3",
        "42",
        "(1 + 2)",
    ]
    .into_iter()
    .enumerate()
    {
        let tree = syntax::parse(code).unwrap_or_else(|error| panic!("{code}: {error}"));
        assert_eq!(tree.statements().len(), 1, "{code}: statement count");
        let statement = &tree.statements()[0];
        let grouped = statement.grouped();
        assert_eq!(grouped.span(), 0..code.len(), "{code}: expression span");
        if !matches!(&statement.kind, Kind::Chain { .. }) {
            assert!(
                matches!(&grouped, Grouped::Operand(node) if std::ptr::eq(*node, statement)),
                "{code}: operand identity"
            );
        }
        let expected = evaluate(&grouped, code);
        vm.capture_errors();
        let source = format!("{code}\n;");
        let value = vm.eval(ScriptMod::new(
            "",
            "",
            &format!("chain_{index}"),
            0,
            0,
            &source,
            &[],
        ));
        let errors = vm.take_errors();
        assert!(errors.is_empty(), "{code}: {errors:?}");
        let actual = if let Some(number) = value.as_number() {
            Value::Number(number)
        } else if let Some(boolean) = value.as_bool() {
            Value::Bool(boolean)
        } else if value.is_nil() {
            Value::Nil
        } else {
            panic!("{code}: unexpected VM value {value:?}");
        };
        assert_eq!(expected, actual, "{code}");
    }

    let code = "(1 + 2) * 3";
    let node = syntax::parse_value(code).unwrap();
    let Grouped::Binary { lhs, .. } = node.grouped() else {
        panic!("{code}: expected a binary expression")
    };
    assert!(
        matches!(
            lhs.as_ref(),
            Grouped::Operand(Node {
                kind: Kind::Paren { .. },
                ..
            })
        ),
        "{code}: parentheses stay one operand"
    );
    assert_eq!(lhs.span(), 0..7, "{code}: parentheses span");

    let vm_parse = |vm: &mut ScriptVm, code: &str| {
        let mut tokenizer = tokenizer::ScriptTokenizer::default();
        tokenizer.tokenize(code, &mut vm.bx.heap);
        tokenizer.finish(&mut vm.bx.heap);
        let mut parser = parser::ScriptParser::default();
        parser.set_emit_errors(false);
        parser.parse(&tokenizer, "chain", (0, 0), &[]);
        parser
    };

    // `...` has no binary rule anywhere: the tree and the VM both refuse it.
    let code = "1 ... 3";
    assert_eq!(operator_order("..."), None, "{code}: `...` is not a chain operator");
    assert!(syntax::parse(code).is_err(), "{code}: syntax must reject it");
    assert!(vm_parse(&mut vm, code).had_error, "{code}: VM parser must reject it");

    // `>>>` is Splash's unsigned shift: compute kernels use it, so the tree, which every
    // Splash file goes through, reads it as one operator at the order of `>>`. The script
    // VM has no unsigned shift: its parser refuses `>>>` with a located error.
    assert_eq!(operator_order(">>>"), operator_order(">>"), "`>>>` binds like `>>`");
    for (code, outer, inner) in [("8 >>> 1 * 2", ">>>", "*"), ("1 << 4 >>> 2", ">>>", "<<"), ("x >>> 2 > 1", ">", ">>>")] {
        let node = syntax::parse_value(code).unwrap_or_else(|e| panic!("{code}: the tree reads `>>>`: {e}"));
        assert_eq!(node.span, 0..code.len(), "{code}: one expression");
        let Grouped::Binary { op, lhs, rhs, .. } = node.grouped() else {
            panic!("{code}: expected a binary expression")
        };
        assert_eq!(op, outer, "{code}: the loosest operator is the root");
        let inner_op = |g: &Grouped| match g {
            Grouped::Binary { op, op_span, .. } => Some((op.to_string(), op_span.clone())),
            Grouped::Operand(_) => None,
        };
        let (found, op_span) = inner_op(&lhs).or_else(|| inner_op(&rhs)).unwrap_or_else(|| panic!("{code}: expected a nested operator"));
        assert_eq!(found, inner, "{code}: precedence of `>>>` against `{inner}`");
        assert_eq!(&code[op_span], inner, "{code}: the operator spans its own bytes");
    }
    let code = "8 >>> 1";
    let parser = vm_parse(&mut vm, code);
    assert!(parser.had_error, "{code}: the VM has no unsigned shift and refuses it");
    let located = parser.diagnostics().first().unwrap_or_else(|| panic!("{code}: the refusal is a diagnostic"));
    // The VM's tokenizer places an operator token at its end, so the column is that of the byte
    // after the glued `>`: within `>>>` and its end, on the first line, naming the `>`.
    assert!(
        located.line == 0 && (2..=5).contains(&located.column) && located.message.contains("Operator(>)"),
        "{code}: the refusal points at the glued `>`: line {} column {}: {}", located.line, located.column, located.message
    );
}
