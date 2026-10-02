//! Per-pass tests on small modules written in the text format.

use super::*;
use ir::*;

fn wat(text: &str) -> Vec<u8> {
    let buf = wast::parser::ParseBuffer::new(text).unwrap();
    let mut module = wast::parser::parse::<wast::Wat>(&buf).unwrap();
    module.encode().unwrap()
}

fn only(set: impl Fn(&mut OptimizeOptions)) -> OptimizeOptions {
    let mut opts = OptimizeOptions {
        strip: false,
        dce: false,
        peephole: false,
        locals: false,
        merge: false,
        compact: false,
        order: false,
        ..OptimizeOptions::default()
    };
    set(&mut opts);
    opts
}

fn optimize(bytes: &[u8], opts: &OptimizeOptions) -> Module {
    let (out, report) = wasm_optimize_checked(bytes, opts).unwrap();
    for pass in &report.passes {
        assert!(pass.reverted.is_none(), "{} reverted: {:?}", pass.name, pass.reverted);
    }
    let module = ir::decode(&out).unwrap();
    ir::validate_module(&module).unwrap();
    module
}

#[test]
fn roundtrip_is_exact() {
    let bytes = wat(r#"(module
        (type (func (param i32) (result i32)))
        (import "env" "f" (func (type 0)))
        (memory 1 2 shared)
        (global (mut i32) (i32.const 0))
        (table 2 funcref)
        (elem (i32.const 0) func 0 1)
        (func (type 0)
            local.get 0
            i32.atomic.load offset=4
            v128.const i32x4 1 2 3 4
            i32x4.extract_lane 2
            i32.add
            i32.const 0 i32.const 0 i32.const 0 memory.fill
            local.get 0
            call_indirect (type 0))
        (data "hello"))"#);
    let module = ir::decode(&bytes).unwrap();
    ir::validate_module(&module).unwrap();
    assert_eq!(encode::encode(&module), bytes);
}

#[test]
fn validator_refuses() {
    let bad = [
        // Wrong operand type.
        r#"(module (func (result i32) i64.const 1))"#,
        // Atomic accesses must be naturally aligned.
        r#"(module (memory 1 1 shared) (func (result i32) i32.const 0 i32.atomic.load align=2))"#,
        // ref.func of an undeclared function.
        r#"(module (func) (func (result funcref) ref.func 0))"#,
        // Lane index out of range.
        r#"(module (func (result i64) v128.const i64x2 0 0 i64x2.extract_lane 2))"#,
        // Branch to a missing label.
        r#"(module (func block br 2 end))"#,
    ];
    for (i, text) in bad.iter().enumerate() {
        let buf = wast::parser::ParseBuffer::new(text).unwrap();
        let mut module = wast::parser::parse::<wast::Wat>(&buf).unwrap();
        // The text encoder checks some of these itself.
        if let Ok(bytes) = module.encode() {
            assert!(wasm_validate(&bytes).is_err(), "case {i} accepted");
        }
    }
    let good = wat(r#"(module (func (result i64) v128.const i64x2 0 0 i64x2.extract_lane 1))"#);
    wasm_validate(&good).unwrap();
}

#[test]
fn dce_removes_unreachable_code_and_imports() {
    let bytes = wat(r#"(module
        (import "env" "used" (func $used))
        (import "env" "unused" (func $unused))
        (import "env" "g" (global $g i32))
        (global $dead (mut i32) (i32.const 1))
        (func $root (export "root") call $used call $a)
        (func $a)
        (func $dead1 call $dead2 global.get $dead drop)
        (func $dead2 call $unused))"#);
    let module = optimize(&bytes, &only(|o| o.dce = true));
    assert_eq!(module.imports.len(), 1);
    assert_eq!(module.funcs.len(), 2);
    assert!(module.globals.is_empty());
    assert_eq!(module.exports[0].index, 1);
}

#[test]
fn dce_keeps_only_callable_table_entries() {
    // Only (i32)->i32 is called indirectly: the () entries become a stub
    // and the stubs at the segment's ends become empty slots.
    let bytes = wat(r#"(module
        (type $t (func (param i32) (result i32)))
        (table 4 funcref)
        (elem (i32.const 0) func $n1 $f $n2 $n3)
        (func $n1) (func $n2) (func $n3)
        (func $f (type $t) local.get 0)
        (func (export "call") (param i32) (result i32)
            local.get 0 local.get 0 call_indirect (type $t)))"#);
    let module = optimize(&bytes, &only(|o| o.dce = true));
    let ElemItems::Funcs(items) = &module.elems[0].items else {
        panic!()
    };
    assert_eq!(items.len(), 1, "{:?}", module.elems);
    assert_eq!(
        module.elems[0].mode,
        ElemMode::Active {
            table: 0,
            offset: vec![Instr::I32Const(1), Instr::End]
        }
    );
    assert_eq!(module.funcs.len(), 2);

    // An exported table keeps every entry.
    let bytes = wat(r#"(module
        (table (export "t") 2 funcref)
        (elem (i32.const 0) func $a $b)
        (func $a) (func $b))"#);
    let module = optimize(&bytes, &only(|o| o.dce = true));
    assert_eq!(module.funcs.len(), 2);
}

#[test]
fn dce_stubs_interior_entries() {
    let bytes = wat(r#"(module
        (type $t (func (param i32) (result i32)))
        (table 3 funcref)
        (elem (i32.const 0) func $f $n $f)
        (func $n (param i64))
        (func $f (type $t) local.get 0)
        (func (export "call") (param i32) (result i32)
            local.get 0 local.get 0 call_indirect (type $t)))"#);
    let module = optimize(&bytes, &only(|o| o.dce = true));
    let ElemItems::Funcs(items) = &module.elems[0].items else {
        panic!()
    };
    assert_eq!(items.len(), 3);
    let stub = items[1] as usize;
    assert_eq!(module.funcs[stub].body, vec![Instr::Unreachable, Instr::End]);
}

#[test]
fn dce_drops_unused_passive_segments() {
    let bytes = wat(r#"(module
        (memory 1)
        (data $used "abc")
        (data $unused "def")
        (func (export "init") i32.const 0 i32.const 0 i32.const 3 memory.init $used))"#);
    let module = optimize(&bytes, &only(|o| o.dce = true));
    assert_eq!(module.datas.len(), 1);
    assert_eq!(module.datas[0].bytes, b"abc");
}

#[test]
fn merge_and_compact() {
    let bytes = wat(r#"(module
        (type $a (func (param i32) (result i32)))
        (type $b (func (param i32) (result i32)))
        (func $x (export "x") (type $a) local.get 0 i32.const 1 i32.add)
        (func $y (export "y") (type $b) local.get 0 i32.const 1 i32.add)
        (func (export "z") (result i32) i32.const 2 call $y))"#);
    let module = optimize(&bytes, &only(|o| o.merge = true));
    assert_eq!(module.funcs.len(), 2);
    assert_eq!(module.exports[0].index, module.exports[1].index);
    assert_eq!(module.types.len(), 2);
    let module = optimize(&bytes, &only(|o| o.compact = true));
    assert_eq!(module.types.len(), 2);
    // The most used type comes first.
    assert_eq!(module.types[0].params, vec![ValType::I32]);
}

#[test]
fn compact_puts_hot_functions_in_one_byte_indices() {
    let mut text = String::from("(module\n");
    for i in 0..200 {
        text.push_str(&format!("(func $f{i} (export \"f{i}\"))\n"));
    }
    text.push_str("(func (export \"hot_caller\")");
    for _ in 0..10 {
        text.push_str(" call $f199");
    }
    text.push_str("))");
    let module = optimize(&wat(&text), &only(|o| o.compact = true));
    let hot = module.exports.iter().find(|e| e.name == "f199").unwrap();
    assert!(hot.index < 128);
}

#[test]
fn strip_keeps_names_on_request() {
    let bytes = wat(r#"(module (func $kept (export "e")) (func $gone) (@custom "extra" "x"))"#);
    let opts = OptimizeOptions {
        keep_names: true,
        ..OptimizeOptions::default()
    };
    let (out, _) = wasm_optimize_checked(&bytes, &opts).unwrap();
    let module = ir::decode(&out).unwrap();
    assert!(module.customs.is_empty());
    let names = module.names.unwrap();
    assert_eq!(names.funcs, vec![(0, "kept".to_string())]);

    let (out, _) = wasm_optimize_checked(&bytes, &OptimizeOptions::default()).unwrap();
    assert!(ir::decode(&out).unwrap().names.is_none());
}

#[test]
fn order_keeps_index_classes_and_behaviour_shape() {
    let mut text = String::from("(module\n");
    for i in 0..140 {
        text.push_str(&format!("(func (export \"f{i}\") (result i32) i32.const {})\n", 139 - i));
    }
    text.push(')');
    let bytes = wat(&text);
    let module = optimize(&bytes, &only(|o| o.order = true));
    // Below 128 nothing moves; above, bodies are sorted.
    assert_eq!(module.exports[0].index, 0);
    let body = |name: &str| {
        let e = module.exports.iter().find(|e| e.name == name).unwrap();
        module.funcs[e.index as usize].body.clone()
    };
    assert_eq!(body("f139"), vec![Instr::I32Const(0), Instr::End]);
    assert_eq!(body("f130"), vec![Instr::I32Const(9), Instr::End]);
    let e = module.exports.iter().find(|e| e.name == "f139").unwrap();
    assert_eq!(e.index, 128);
}

#[test]
fn invalid_pass_output_is_reverted() {
    // A pass's invalid result is caught by the per-pass validation; here
    // the input itself is fine, so nothing may be reverted.
    let bytes = wat(r#"(module (func (export "f") (param i32) (result i32)
        (local i32 i32)
        local.get 0 local.set 1 local.get 1 local.set 2 local.get 2))"#);
    let (_, report) = wasm_optimize_checked(&bytes, &OptimizeOptions::default()).unwrap();
    assert!(report.passes.iter().all(|pass| pass.reverted.is_none()));
    assert!(report.output_bytes < report.input_bytes);
}

#[test]
fn units_never_used_are_cut_with_their_data() {
    // app::widget registers (script_mod, param i32) and draws; only its
    // registration ran. app::main and app::kept ran after.
    let bytes = wat(r#"(module
        (memory (export "memory") 1)
        (table 2 funcref)
        (elem (i32.const 0) func $_RNvNtCs1_3app6widget4draw $_RNvNtCs1_3app4kept4used)
        (func $_RNvNtCs1_3app6widget10script_mod (param i32)
            i32.const 1024 i32.const 12 call $_RNvNtCs1_3app6widget4draw drop)
        (func $_RNvNtCs1_3app6widget4draw (param i32 i32) (result i32)
            local.get 0 i32.load8_u)
        (func $_RNvNtCs1_3app4kept4used (param i32 i32) (result i32)
            i32.const 2048 i32.load8_u)
        (func $_RNvNtCs1_3app4main3run (export "run") (param i32) (result i32)
            local.get 0 call $_RNvNtCs1_3app6widget10script_mod
            local.get 0
            if (result i32)
                i32.const 1 i32.const 2 call $_RNvNtCs1_3app6widget4draw
            else
                i32.const 1 i32.const 2 call $_RNvNtCs1_3app4kept4used
            end)
        (data (i32.const 1024) "SPLASHSOURCE...........................................................................")
        (data (i32.const 2048) "KEEP"))"#);
    let coverage = units::Coverage::parse(
        "2\t_RNvNtCs1_3app4main3run\n2\t_RNvNtCs1_3app4kept4used\n1\t_RNvNtCs1_3app6widget10script_mod\n",
    );
    let opts = OptimizeOptions { coverage: Some(coverage), keep_names: true, ..OptimizeOptions::default() };
    let (out, report) = wasm_optimize_checked(&bytes, &opts).unwrap();
    assert!(report.passes.iter().all(|p| p.reverted.is_none()), "{}", report.to_text());
    assert!(report.units.as_ref().unwrap().contains("CUT"), "{:?}", report.units);
    let module = ir::decode(&out).unwrap();
    ir::validate_module(&module).unwrap();
    let names: Vec<&str> = module.names.as_ref().unwrap().funcs.iter().map(|(_, n)| n.as_str()).collect();
    assert!(!names.iter().any(|n| n.contains("6widget")), "{names:?}");
    let data: Vec<u8> = module.datas.iter().flat_map(|d| d.bytes.clone()).collect();
    assert!(!data.windows(6).any(|w| w == b"SPLASH"), "the stripped unit's text stays");
    assert!(data.windows(4).any(|w| w == b"KEEP"));
    // The registration call is gone (its argument dropped); the draw call
    // traps; the kept call stays.
    let run = module.funcs.iter().find(|f| f.body.iter().any(|i| matches!(i, Instr::If(_)))).unwrap();
    assert!(run.body.contains(&Instr::Unreachable));
    assert_eq!(run.body.iter().filter(|i| matches!(i, Instr::Call(_))).count(), 1);

    // The instrumented module validates and marks entries in its own memory.
    let instrumented = wasm_instrument_coverage(&bytes, &["main::run".to_string()]).unwrap();
    let module = ir::decode(&instrumented).unwrap();
    assert!(module.exports.iter().any(|e| e.name == units::COVERAGE_EXPORT && e.kind == ExternKind::Memory));
}

#[test]
fn panics_become_traps_with_their_sites() {
    // `get` checks its index and panics through `$panic` (which logs and
    // never returns) with a message and a location in the data.
    let bytes = wat(r#"(module
        (import "env" "log" (func $log (param i32 i32)))
        (memory (export "memory") 1)
        (func $panic (param i32 i32 i32)
            local.get 0 local.get 1 call $log
            unreachable)
        (func $get (export "get") (param i32) (result i32)
            local.get 0
            i32.const 4
            i32.ge_u
            if
                i32.const 120 i32.const 18 i32.const 160
                call $panic
                unreachable
            end
            local.get 0
            i32.const 2
            i32.mul)
        (data (i32.const 100) "src/lib.rs")
        (data (i32.const 120) "index out of range")
        (data (i32.const 160) "\64\00\00\00\0a\00\00\00\0c\00\00\00\05\00\00\00"))"#);
    let opts = OptimizeOptions { panic_trap: true, symbols: true, ..OptimizeOptions::default() };
    let (out, report) = wasm_optimize_checked(&bytes, &opts).unwrap();
    assert!(report.passes.iter().all(|p| p.reverted.is_none()), "{}", report.to_text());
    let module = ir::decode(&out).unwrap();
    // The panic function and the import only it called are gone; the
    // names went too.
    assert_eq!(module.funcs.len(), 1);
    assert!(module.imports.is_empty() && module.names.is_none());
    assert!(!module.funcs[0].body.iter().any(|i| matches!(i, Instr::Call(_))));
    // The message, the location and its file name are cleared.
    let data: Vec<u8> = module.datas.iter().flat_map(|d| d.bytes.clone()).collect();
    assert!(data.iter().all(|b| *b == 0), "{data:?}");

    let symbols = report.symbols.unwrap();
    assert!(symbols.contains("F 0 get"), "{symbols}");
    let site = symbols.lines().find(|l| l.starts_with("P ")).expect("a panic site");
    assert!(site.ends_with("panic | index out of range | src/lib.rs:12:5"), "{site}");
    // The offset is the trap's `unreachable` in the shipped bytes.
    let at = usize::from_str_radix(site.split(' ').nth(1).unwrap().trim_start_matches("0x"), 16).unwrap();
    assert_eq!(out[at], 0x00);

    // In range it still answers; out of range it traps.
    let engine = makepad_stitch::Engine::new();
    let mut store = makepad_stitch::Store::new(engine.clone());
    let module = makepad_stitch::Module::new(&engine, &out).unwrap();
    let instance = makepad_stitch::Linker::new().instantiate(&mut store, &module).unwrap();
    let get = instance.exported_func("get").unwrap();
    let mut result = [makepad_stitch::Val::I32(0)];
    get.call(&mut store, &[makepad_stitch::Val::I32(3)], &mut result).unwrap();
    assert_eq!(result[0].to_i32(), Some(6));
    assert!(get.call(&mut store, &[makepad_stitch::Val::I32(9)], &mut result).is_err());
}

#[test]
fn waits_answer_at_once_where_a_thread_may_not_block() {
    let bytes = wat(r#"(module
        (memory 1 2 shared)
        (func (export "w32") (param i32 i32) (result i32)
            local.get 0 local.get 1 i64.const -1 memory.atomic.wait32 offset=8)
        (func (export "w64") (param i32 i64) (result i32)
            local.get 0 local.get 1 i64.const -1 memory.atomic.wait64)
        (func (export "n") (param i32) (result i32)
            local.get 0 i32.const 1 memory.atomic.notify))"#);
    let module = optimize(&bytes, &only(|_| {}));
    let waits = |m: &Module| m.funcs.iter().flat_map(|f| f.body.iter()).filter(|i| matches!(i, Instr::Atomic(1 | 2, _))).count();
    // The two waits now sit only in the two helpers, behind the flag.
    assert_eq!(waits(&module), 2);
    assert_eq!(module.funcs.len(), 5);
    for helper in &module.funcs[3..] {
        assert!(matches!(helper.body[0], Instr::GlobalGet(_)), "{:?}", helper.body);
    }
    let flag = module.exports.iter().find(|e| e.name == waits::CANNOT_BLOCK_EXPORT).expect("the flag is exported");
    assert_eq!(flag.kind, ExternKind::Global);
    assert!(module.globals[flag.index as usize].ty.mutable);
    // Guarding again changes nothing.
    let again = wasm_guard_waits(&encode::encode(&module)).unwrap();
    assert_eq!(again, encode::encode(&module));
    // An unshared memory has nothing to guard.
    let plain = optimize(&wat(r#"(module (memory 1) (func (result i32) i32.const 0))"#), &only(|_| {}));
    assert!(plain.exports.is_empty() && plain.globals.is_empty());
}
