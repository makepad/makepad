use makepad_script::syntax::{self, Kind};
use makepad_script::tokenizer::{lex_with, ScriptToken, ScriptTokenPos, ScriptTokenTag, ScriptTokenizer};
use makepad_script::*;

fn tokens(source: &str, units: &UnitTable, heap: &mut ScriptHeap) -> Vec<ScriptTokenPos> {
    let mut tokenizer = ScriptTokenizer::with_units(units.clone());
    tokenizer.tokenize(source, heap);
    tokenizer.finish(heap);
    tokenizer.tokens
}

fn same_tokens(actual: &[ScriptTokenPos], expected: &[ScriptTokenPos], property: &str) {
    assert_eq!(actual.len(), expected.len(), "{property}: token count");
    for (a, b) in actual.iter().zip(expected) {
        assert_eq!(a.token.kind(), b.token.kind(), "{property}: token kind");
        assert_eq!(a.token.number_f64(), b.token.number_f64(), "{property}: number value");
        assert_eq!(a.token.as_identifier(), b.token.as_identifier(), "{property}: identifier");
        assert_eq!(a.token.operator_or_separator(), b.token.operator_or_separator(), "{property}: operator");
        assert_eq!(a.span(), b.span(), "{property}: span");
        assert_eq!(a.pos(), b.pos(), "{property}: pos");
        assert_eq!(a.unit, b.unit, "{property}: unit index");
        assert_eq!(a.preceded_by_newline, b.preceded_by_newline, "{property}: newline");
        assert_eq!(a.preceded_by_space, b.preceded_by_space, "{property}: space");
    }
}

fn close(actual: f64, expected: f64, property: &str) {
    assert!((actual - expected).abs() < 1e-12, "{property}: {actual} != {expected}");
}

#[test]
fn registered_units_across_tokenizer_syntax_and_vm() {
    let standard = UnitTable::standard();
    let t = standard.clone().with("us", 1.0, 1e6).with("inch", 254.0, 10000.0).with("ft", 3048.0, 10000.0)
        .with("hr", 3600.0, 1.0).with("min", 60.0, 1.0).with("mi", 1609344.0, 1000.0).with("em", 16.0, 1.0);
    let mut heap = ScriptHeap::default();
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    assert_eq!(vm.unit_table(), standard, "VM defaults to standard units");
    vm.set_unit_table(&t);
    assert_eq!(vm.unit_table(), t, "VM stores a copy of the registered table");
    assert_eq!(UnitTable::default(), standard, "table default is standard");
    let replaced = t.clone().with("inch", 2.0, 1.0);
    assert_eq!(replaced.lookup("inch"), t.lookup("inch"), "replacement keeps indices stable");
    assert_eq!(replaced.entries().len(), t.entries().len(), "replacement adds no duplicate");
    assert_eq!(replaced.entry(replaced.lookup("inch").unwrap()).to_base(3.0), 6.0, "replacement updates the ratio");
    let mm = standard.entry(standard.lookup("mm").unwrap());
    assert_eq!(mm.literal("18") * 1000.0, 18.0, "decimal units divide exactly");
    assert_eq!((mm.literal("4.2"), mm.literal("42e-1")), (4.2e-3, 4.2e-3), "decimal units shift the exponent of the digits");
    let k = UnitTable::standard().with("kohm", 1000.0, 1.0).with("uF", 1.0, 1e6);
    assert_eq!(k.entry(k.lookup("kohm").unwrap()).literal("8.06"), 8060.0, "8.06kohm is exactly 8060 (8.06 * 1000 is not)");
    assert_eq!(k.entry(k.lookup("uF").unwrap()).literal("0.1"), 1e-7, "0.1uF is the double nearest 1e-7");
    assert!(t.has_suffix_starting_with('i'), "type suffix can begin a registered unit");
    assert!(!standard.has_suffix_starting_with('i'), "standard table preserves immediate type suffixes");

    for (source, expected) in [
        ("2us", 2e-6), ("3inch", 0.0762), ("1.5ft", 0.4572), ("2hr", 7200.0),
        ("1min", 60.0), ("1mi", 1609.344), ("150mm", 0.15),
        ("90deg", std::f64::consts::FRAC_PI_2), ("90°", std::f64::consts::FRAC_PI_2),
        ("1e3", 1000.0), ("2.5e-3mm", 2.5e-6),
    ] {
        let lexed = tokens(source, &t, &mut heap);
        assert_eq!(lexed.len(), 1, "{source}: a single numeric token");
        assert_eq!(lexed[0].token.kind(), ScriptTokenTag::F64, "{source}: unit/exponent emits F64");
        close(lexed[0].token.number_f64().unwrap(), expected, &format!("{source}: tokenizer scale"));
        let value = vm.eval(ScriptMod::new("", "", "units", 0, 0, source, &[]));
        close(value.as_number().expect("VM unit literal is a number"), expected, &format!("{source}: VM scale"));
        assert!(vm.take_errors().is_empty(), "{source}: VM evaluates without errors");
    }

    for (source, number, suffix) in [("1Mi", 1, "Mi"), ("5V", 5, "V")] {
        let lexed = tokens(source, &t, &mut heap);
        assert_eq!(lexed.len(), 2, "{source}: unknown suffix is a separate identifier");
        assert_eq!(lexed[0].token.as_u40(), Some(number), "{source}: unknown suffix keeps the number");
        assert_eq!(lexed[1].token.as_identifier(), Some(LiveId::from_str(suffix)), "{source}: case-sensitive identifier");
        assert!(lexed.iter().all(|token| token.unit.is_none()), "{source}: no registered unit");
        assert_eq!((lexed[0].span(), lexed[1].span()), (0..1, 1..source.len()), "{source}: number and identifier spans do not overlap");
        same_tokens(&lexed, &tokens(source, &standard, &mut heap), "unknown suffix preserves standard tokens");
    }
    let spaced = syntax::parse_with("let v = [2.5V 6V]", &standard).expect("unregistered suffixes parse");
    assert_eq!(spaced.print(), "let v = [2.5V 6V]", "unregistered suffixes print back losslessly");
    for (source, kind, value) in [
        ("1u", ScriptTokenTag::U32, 1.0), ("2i", ScriptTokenTag::I32, 2.0),
        ("3f", ScriptTokenTag::F32, 3.0), ("4h", ScriptTokenTag::F16, 4.0),
    ] {
        let lexed = tokens(source, &t, &mut heap);
        assert_eq!(lexed.len(), 1, "{source}: one typed token");
        assert_eq!(lexed[0].token.kind(), kind, "{source}: type suffix kind");
        assert_eq!(lexed[0].token.number_f64(), Some(value), "{source}: type suffix value");
        same_tokens(&lexed, &tokens(source, &standard, &mut heap), "typed suffix preserves spans and pos");
    }
    for source in ["1ux", "2ix", "3fx", "4hx", "1_000ux", "1em", "0x1f"] {
        same_tokens(&tokens(source, &t, &mut heap), &tokens(source, &standard, &mut heap), source);
    }
    let unknown = tokens("1ux", &t, &mut heap);
    assert!(matches!(unknown[0].token, ScriptToken::U32(1)), "unknown u-prefix falls back to U32");
    assert_eq!(unknown[1].token.as_identifier(), Some(LiveId::from_str("x")), "unknown u-prefix keeps the remaining identifier");
    let inch = tokens("2inch", &t, &mut heap);
    assert_eq!(inch[0].span(), 0..5, "unit span includes its suffix");
    assert_eq!(t.entry(inch[0].unit.expect("registered unit index")).suffix, "inch", "unit index names the suffix");
    assert_eq!(lex_with("2inch", &t)[0].span(), 0..5, "lossless lexer includes the suffix");

    for source in ["x = 2inch + 1", "1ux", "2.5e-3mm", "90°", "1min", "1em", "0x1f"] {
        let whole = tokens(source, &t, &mut heap);
        for split in source.char_indices().map(|(at, _)| at).chain(std::iter::once(source.len())) {
            let mut streamed = ScriptTokenizer::with_units(t.clone());
            streamed.tokenize(&source[..split], &mut heap);
            streamed.push_pending_token(&mut heap);
            streamed.tokenize(&source[split..], &mut heap);
            streamed.finish(&mut heap);
            same_tokens(&streamed.tokens, &whole, &format!("{source}: streaming split at {split}"));
        }
    }
    let mut cleared = ScriptTokenizer::with_units(t.clone());
    cleared.tokenize("2in", &mut heap);
    cleared.clear();
    assert_eq!(cleared.units(), &t, "clear retains the table");
    cleared.tokenize("2inch", &mut heap);
    cleared.finish(&mut heap);
    same_tokens(&cleared.tokens, &inch, "clear resets a pending suffix");

    let tree = syntax::parse_with("let a = 2inch", &t).expect("registered unit file parses");
    let loc = tree.find_let("a").expect("binding exists").child(0);
    let node = tree.node(&loc).expect("binding value exists");
    assert_eq!(node.kind, Kind::Number(0.0508), "syntax stores the scaled number");
    assert_eq!(node.span, 8..13, "syntax number spans the entire unit literal");
    let edited = tree.replace(&loc, "3inch").expect("replace reparses with the stored table").syntax;
    assert_eq!(edited.source(), "let a = 3inch", "edit changes only the literal");
    let Kind::Number(value) = edited.node(&loc).unwrap().kind else { panic!("edited value stays a number") };
    close(value, 0.0762, "edit keeps registered units");
    assert_eq!(syntax::parse_value_with("2inch", &t).unwrap().kind, Kind::Number(0.0508), "value parser uses registered units");

    let expression = "2inch + 1ft";
    let value = vm.eval(ScriptMod::new("", "", "expression", 0, 0, expression, &[]));
    close(value.as_number().expect("VM unit expression is numeric"), 0.3556, "VM adds scaled literals");
    vm.set_unit_table(&replaced);
    let cached = vm.eval(ScriptMod::new("", "", "expression", 0, 0, expression, &[]));
    close(cached.as_number().unwrap(), 0.3556, "new table leaves already-parsed bodies untouched");
    let reparsed = vm.eval(ScriptMod::new("", "", "expression", 0, 0, "3inch + 1ft", &[]));
    close(reparsed.as_number().unwrap(), 6.3048, "changed body uses the new VM table");
    assert!(vm.take_errors().is_empty(), "VM expressions evaluate without errors");
}

#[test]
fn registered_percent_literals_preserve_modulo() {
    let standard = UnitTable::standard();
    let t = standard.clone().with("%", 1.0, 100.0);
    let percent = t.lookup("%").expect("percent suffix is registered");
    let mut heap = ScriptHeap::default();
    let literals = [
        ("1%,", 0.01, 2, 2),
        ("1%)", 0.01, 2, 2),
        ("2.5% ", 0.025, 4, 1),
        ("1%}", 0.01, 2, 2),
        ("1%]", 0.01, 2, 2),
        ("1%\n", 0.01, 2, 1),
        ("5%", 0.05, 2, 1),
    ];
    for (source, expected, end, count) in literals {
        let lexed = tokens(source, &t, &mut heap);
        assert_eq!(lexed.len(), count, "{source:?}: percent literal token count");
        assert_eq!(lexed[0].token.kind(), ScriptTokenTag::F64, "{source:?}: percent literal is F64");
        assert_eq!(lexed[0].unit, Some(percent), "{source:?}: percent literal unit index");
        close(lexed[0].token.number_f64().unwrap(), expected, &format!("{source:?}: percent literal scale"));
        assert_eq!(lexed[0].token.number_f64(), Some(expected), "{source:?}: percent literal rounds the decimal digits exactly");
        assert_eq!(lexed[0].span(), 0..end, "{source:?}: percent literal span includes its suffix");
    }

    let modulo = [
        "10%3", "10%x", "10%(2)", "a%b", "10 % 3", "10 %3", "10%=3",
        "10%_x", "10%$x", "10%é", "é\n10%=3", "2.5%3", "1e2%3", "1_000%3", "0x10%)", "0x10%",
    ];
    for source in modulo {
        same_tokens(&tokens(source, &t, &mut heap), &tokens(source, &standard, &mut heap),
            &format!("{source:?}: registered percent preserves modulo"));
    }

    for source in literals.iter().map(|&(source, _, _, _)| source).chain(modulo) {
        let whole = tokens(source, &t, &mut heap);
        for split in source.char_indices().map(|(at, _)| at).chain(std::iter::once(source.len())) {
            let mut streamed = ScriptTokenizer::with_units(t.clone());
            streamed.tokenize(&source[..split], &mut heap);
            streamed.push_pending_token(&mut heap);
            streamed.tokenize(&source[split..], &mut heap);
            streamed.finish(&mut heap);
            same_tokens(&streamed.tokens, &whole, &format!("{source:?}: percent streaming split at {split}"));
        }
    }

    let unregistered = tokens("1%)", &standard, &mut heap);
    assert_eq!(unregistered.len(), 3, "unregistered percent keeps three tokens");
    // Plain integral literals retain the tokenizer's existing U40 representation.
    assert_eq!(unregistered[0].token.kind(), ScriptTokenTag::U40, "unregistered percent keeps the number kind");
    close(unregistered[0].token.number_f64().unwrap(), 1.0, "unregistered percent keeps the number value");
    assert!(unregistered.iter().all(|token| token.unit.is_none()), "unregistered percent gives no token a unit");
    assert_eq!(unregistered[1].token.kind(), ScriptTokenTag::Operator, "unregistered percent stays an operator");
    assert_eq!(unregistered[1].token.operator_or_separator(), Some(LiveId::from_str("%")), "unregistered percent keeps the modulo operator");
    assert_eq!(unregistered[2].token.kind(), ScriptTokenTag::CloseRound, "unregistered percent keeps the closing parenthesis");
    assert_eq!((unregistered[0].span(), unregistered[1].span(), unregistered[2].span()), (0..1, 1..2, 2..3), "unregistered percent keeps separate spans");

    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    vm.set_unit_table(&t);
    let value = vm.eval(ScriptMod::new("", "", "percent_units", 0, 0, "[1%, 10%3]", &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "VM evaluates percent and modulo without errors: {errors:?}");
    let array = value.as_array().expect("VM percent and modulo expression returns an array");
    let heap = vm.bx.heap_ref();
    assert_eq!(heap.array_len(array), 2, "VM percent and modulo array has two elements");
    close(heap.array_index(array, 0, NoTrap).as_number().expect("VM percent value is numeric"), 0.01, "VM scales the percent literal");
    close(heap.array_index(array, 1, NoTrap).as_number().expect("VM modulo value is numeric"), 1.0, "VM evaluates the modulo expression");
}
