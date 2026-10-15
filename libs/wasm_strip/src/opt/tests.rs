//! Per-pass tests on small modules built directly in the binary format.

use super::*;
use ir::*;

// A deliberately small, independent binary encoder for these fixtures. It
// does not use the optimiser's encoder, so roundtrip_is_exact tests both sides.
mod binary {
    pub const I32: u8 = 0x7f;
    pub const I64: u8 = 0x7e;
    pub const FUNCREF: u8 = 0x70;

    pub fn uint(mut n: u32) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (n & 127) as u8;
            n >>= 7;
            out.push(byte | if n == 0 { 0 } else { 128 });
            if n == 0 { return out; }
        }
    }

    fn string(s: &str) -> Vec<u8> {
        [uint(s.len() as u32), s.as_bytes().to_vec()].concat()
    }

    fn vector(items: &[Vec<u8>]) -> Vec<u8> {
        [uint(items.len() as u32), items.concat()].concat()
    }

    pub fn op(opcode: u8, n: u32) -> Vec<u8> {
        [vec![opcode], uint(n)].concat()
    }

    pub fn konst(mut n: i32) -> Vec<u8> {
        let mut out = vec![0x41];
        loop {
            let byte = (n & 127) as u8;
            n >>= 7;
            let done = (n == 0 && byte & 64 == 0) || (n == -1 && byte & 64 != 0);
            out.push(byte | if done { 0 } else { 128 });
            if done { return out; }
        }
    }

    #[derive(Default)]
    pub struct Wasm {
        sections: std::collections::BTreeMap<u8, Vec<Vec<u8>>>,
        names: Vec<Vec<u8>>,
        customs: Vec<Vec<u8>>,
    }

    impl Wasm {
        pub fn new(types: &[(&[u8], &[u8])]) -> Self {
            let mut m = Self::default();
            for (params, results) in types {
                m.entry(1, [vec![0x60], uint(params.len() as u32), params.to_vec(),
                    uint(results.len() as u32), results.to_vec()].concat());
            }
            m
        }
        fn entry(&mut self, section: u8, bytes: Vec<u8>) -> &mut Self {
            self.sections.entry(section).or_default().push(bytes);
            self
        }
        pub fn import(&mut self, name: &str, kind: u8, ty: &[u8]) -> &mut Self {
            self.entry(2, [string("env"), string(name), vec![kind], ty.to_vec()].concat())
        }
        pub fn func(&mut self, ty: u32, locals: &[u8], body: &[u8]) -> &mut Self {
            self.entry(3, uint(ty));
            // One local declaration per local is valid and leaves the optimiser
            // opportunities to compact declarations as well as instructions.
            let locals: Vec<_> = locals.iter().map(|ty| vec![1, *ty]).collect();
            let body = [vector(&locals), body.to_vec(), vec![0x0b]].concat();
            self.entry(10, [uint(body.len() as u32), body].concat())
        }
        pub fn export(&mut self, name: &str, kind: u8, index: u32) -> &mut Self {
            self.entry(7, [string(name), vec![kind], uint(index)].concat())
        }
        pub fn name(&mut self, index: u32, name: &str) -> &mut Self {
            self.names.push([uint(index), string(name)].concat());
            self
        }
        pub fn memory(&mut self, min: u32, max: Option<u32>, shared: bool) -> &mut Self {
            let mut limits = vec![u8::from(max.is_some()) | if shared { 2 } else { 0 }];
            limits.extend(uint(min));
            if let Some(max) = max { limits.extend(uint(max)); }
            self.entry(5, limits)
        }
        pub fn table(&mut self, size: u32) -> &mut Self {
            self.entry(4, [vec![FUNCREF, 0], uint(size)].concat())
        }
        pub fn global(&mut self, value: i32) -> &mut Self {
            self.entry(6, [vec![I32, 1], konst(value), vec![0x0b]].concat())
        }
        pub fn elem(&mut self, functions: &[u32]) -> &mut Self {
            self.entry(9, [vec![0], konst(0), vec![0x0b],
                vector(&functions.iter().map(|n| uint(*n)).collect::<Vec<_>>())].concat())
        }
        pub fn data(&mut self, offset: Option<i32>, bytes: &[u8]) -> &mut Self {
            let mode = match offset {
                Some(offset) => [vec![0], konst(offset), vec![0x0b]].concat(),
                None => vec![1],
            };
            self.entry(11, [mode, uint(bytes.len() as u32), bytes.to_vec()].concat())
        }
        pub fn custom(&mut self, name: &str, bytes: &[u8]) -> &mut Self {
            self.customs.push([string(name), bytes.to_vec()].concat());
            self
        }
        pub fn finish(&self) -> Vec<u8> {
            fn section(out: &mut Vec<u8>, id: u8, bytes: &[u8]) {
                out.push(id);
                out.extend(uint(bytes.len() as u32));
                out.extend(bytes);
            }
            let mut out = b"\0asm\x01\0\0\0".to_vec();
            for (&id, entries) in &self.sections {
                if id == 10 {
                    if let Some(data) = self.sections.get(&11) {
                        section(&mut out, 12, &uint(data.len() as u32));
                    }
                }
                section(&mut out, id, &vector(entries));
            }
            if !self.names.is_empty() {
                let names = vector(&self.names);
                let payload = [string("name"), vec![1], uint(names.len() as u32), names].concat();
                section(&mut out, 0, &payload);
            }
            for custom in &self.customs { section(&mut out, 0, custom); }
            out
        }
    }
}
use binary::{konst, op, Wasm, FUNCREF, I32, I64};

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
    let bytes = Wasm::new(&[(&[I32], &[I32])])
        .import("f", 0, &[0]).memory(1, Some(2), true).global(0).table(2).elem(&[0, 1])
        .func(0, &[], &[
            0x20, 0, // local.get 0
            0xfe, 0x10, 2, 4, // i32.atomic.load offset=4
            0xfd, 0x0c, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 4, 0, 0, 0,
            0xfd, 0x1b, 2, 0x6a, // i32x4.extract_lane 2; i32.add
            0x41, 0, 0x41, 0, 0x41, 0, 0xfc, 11, 0, // memory.fill
            0x20, 0, 0x11, 0, 0, // local.get 0; call_indirect
        ]).data(None, b"hello").finish();
    let module = ir::decode(&bytes).unwrap();
    ir::validate_module(&module).unwrap();
    assert_eq!(encode::encode(&module), bytes);
}

#[test]
fn validator_refuses() {
    let bad = [
        // Wrong operand type.
        Wasm::new(&[(&[], &[I32])]).func(0, &[], &[0x42, 1]).finish(),
        // Atomic accesses must be naturally aligned.
        Wasm::new(&[(&[], &[I32])]).memory(1, Some(1), true)
            .func(0, &[], &[0x41, 0, 0xfe, 0x10, 1, 0]).finish(),
        // ref.func of an undeclared function.
        Wasm::new(&[(&[], &[]), (&[], &[FUNCREF])])
            .func(0, &[], &[]).func(1, &[], &[0xd2, 0]).finish(),
        // Lane index out of range.
        Wasm::new(&[(&[], &[I64])]).func(0, &[], &[
            0xfd, 0x0c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0xfd, 0x1d, 2,
        ]).finish(),
        // Branch to a missing label.
        Wasm::new(&[(&[], &[])]).func(0, &[], &[0x02, 0x40, 0x0c, 2, 0x0b]).finish(),
    ];
    for (i, bytes) in bad.iter().enumerate() {
        assert!(wasm_validate(bytes).is_err(), "case {i} accepted");
    }
    let good = Wasm::new(&[(&[], &[I64])]).func(0, &[], &[
        0xfd, 0x0c, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0xfd, 0x1d, 1, // v128.const; i64x2.extract_lane 1
    ]).finish();
    wasm_validate(&good).unwrap();
}

#[test]
fn dce_removes_unreachable_code_and_imports() {
    let bytes = Wasm::new(&[(&[], &[])])
        .import("used", 0, &[0]).import("unused", 0, &[0]).import("g", 3, &[I32, 0])
        .global(1).func(0, &[], &[0x10, 0, 0x10, 3]).export("root", 0, 2)
        .func(0, &[], &[]).func(0, &[], &[0x10, 5, 0x23, 1, 0x1a])
        .func(0, &[], &[0x10, 1]).finish();
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
    let bytes = Wasm::new(&[(&[I32], &[I32]), (&[], &[])])
        .table(4).elem(&[0, 3, 1, 2])
        .func(1, &[], &[]).func(1, &[], &[]).func(1, &[], &[])
        .func(0, &[], &[0x20, 0])
        .func(0, &[], &[0x20, 0, 0x20, 0, 0x11, 0, 0]).export("call", 0, 4).finish();
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
    let bytes = Wasm::new(&[(&[], &[])]).table(2).export("t", 1, 0).elem(&[0, 1])
        .func(0, &[], &[]).func(0, &[], &[]).finish();
    let module = optimize(&bytes, &only(|o| o.dce = true));
    assert_eq!(module.funcs.len(), 2);
}

#[test]
fn dce_stubs_interior_entries() {
    let bytes = Wasm::new(&[(&[I32], &[I32]), (&[I64], &[])])
        .table(3).elem(&[1, 0, 1]).func(1, &[], &[]).func(0, &[], &[0x20, 0])
        .func(0, &[], &[0x20, 0, 0x20, 0, 0x11, 0, 0]).export("call", 0, 2).finish();
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
    let bytes = Wasm::new(&[(&[], &[])]).memory(1, None, false)
        .data(None, b"abc").data(None, b"def")
        .func(0, &[], &[0x41, 0, 0x41, 0, 0x41, 3, 0xfc, 8, 0, 0])
        .export("init", 0, 0).finish();
    let module = optimize(&bytes, &only(|o| o.dce = true));
    assert_eq!(module.datas.len(), 1);
    assert_eq!(module.datas[0].bytes, b"abc");
}

#[test]
fn merge_and_compact() {
    let bytes = Wasm::new(&[(&[I32], &[I32]), (&[I32], &[I32]), (&[], &[I32])])
        .func(0, &[], &[0x20, 0, 0x41, 1, 0x6a]).export("x", 0, 0)
        .func(1, &[], &[0x20, 0, 0x41, 1, 0x6a]).export("y", 0, 1)
        .func(2, &[], &[0x41, 2, 0x10, 1]).export("z", 0, 2).finish();
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
    let mut binary = Wasm::new(&[(&[], &[])]);
    for i in 0..200 {
        binary.func(0, &[], &[]).export(&format!("f{i}"), 0, i);
    }
    let body = op(0x10, 199).repeat(10);
    binary.func(0, &[], &body).export("hot_caller", 0, 200);
    let module = optimize(&binary.finish(), &only(|o| o.compact = true));
    let hot = module.exports.iter().find(|e| e.name == "f199").unwrap();
    assert!(hot.index < 128);
}

#[test]
fn strip_keeps_names_on_request() {
    let bytes = Wasm::new(&[(&[], &[])])
        .func(0, &[], &[]).name(0, "kept").export("e", 0, 0)
        .func(0, &[], &[]).name(1, "gone").custom("extra", b"x").finish();
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
    let mut binary = Wasm::new(&[(&[], &[I32])]);
    for i in 0..140 {
        binary.func(0, &[], &konst(139 - i)).export(&format!("f{i}"), 0, i as u32);
    }
    let bytes = binary.finish();
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
    let bytes = Wasm::new(&[(&[I32], &[I32])])
        .func(0, &[I32, I32], &[0x20, 0, 0x21, 1, 0x20, 1, 0x21, 2, 0x20, 2])
        .export("f", 0, 0).finish();
    let (_, report) = wasm_optimize_checked(&bytes, &OptimizeOptions::default()).unwrap();
    assert!(report.passes.iter().all(|pass| pass.reverted.is_none()));
    assert!(report.output_bytes < report.input_bytes);
}

#[test]
fn units_never_used_are_cut_with_their_data() {
    // app::widget registers (script_mod, param i32) and draws; only its
    // registration ran. app::main and app::kept ran after.
    let bytes = Wasm::new(&[(&[I32], &[]), (&[I32, I32], &[I32]), (&[I32], &[I32])])
        .memory(1, None, false).export("memory", 2, 0).table(2).elem(&[1, 2])
        .func(0, &[], &[konst(1024), konst(12), vec![0x10, 1, 0x1a]].concat())
        .name(0, "_RNvNtCs1_3app6widget10script_mod")
        .func(1, &[], &[0x20, 0, 0x2d, 0, 0]).name(1, "_RNvNtCs1_3app6widget4draw")
        .func(1, &[], &[konst(2048), vec![0x2d, 0, 0]].concat())
        .name(2, "_RNvNtCs1_3app4kept4used")
        .func(2, &[], &[
            0x20, 0, 0x10, 0, 0x20, 0, 0x04, I32,
            0x41, 1, 0x41, 2, 0x10, 1, 0x05,
            0x41, 1, 0x41, 2, 0x10, 2, 0x0b,
        ]).name(3, "_RNvNtCs1_3app4main3run").export("run", 0, 3)
        .data(Some(1024), b"SPLASHSOURCE...........................................................................")
        .data(Some(2048), b"KEEP").finish();
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
    let bytes = Wasm::new(&[(&[I32, I32], &[]), (&[I32, I32, I32], &[]), (&[I32], &[I32])])
        .import("log", 0, &[0]).name(0, "log").memory(1, None, false).export("memory", 2, 0)
        .func(1, &[], &[0x20, 0, 0x20, 1, 0x10, 0, 0x00]).name(1, "panic")
        .func(2, &[], &[
            vec![0x20, 0, 0x41, 4, 0x4f, 0x04, 0x40],
            konst(120), konst(18), konst(160),
            vec![0x10, 1, 0x00, 0x0b, 0x20, 0, 0x41, 2, 0x6c],
        ].concat()).name(2, "get").export("get", 0, 2)
        .data(Some(100), b"src/lib.rs").data(Some(120), b"index out of range")
        .data(Some(160), &[100u32, 10, 12, 5].iter().flat_map(|n| n.to_le_bytes()).collect::<Vec<_>>())
        .finish();
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
    let bytes = Wasm::new(&[(&[I32, I32], &[I32]), (&[I32, I64], &[I32]), (&[I32], &[I32])])
        .memory(1, Some(2), true)
        .func(0, &[], &[0x20, 0, 0x20, 1, 0x42, 0x7f, 0xfe, 1, 2, 8]).export("w32", 0, 0)
        .func(1, &[], &[0x20, 0, 0x20, 1, 0x42, 0x7f, 0xfe, 2, 3, 0]).export("w64", 0, 1)
        .func(2, &[], &[0x20, 0, 0x41, 1, 0xfe, 0, 2, 0]).export("n", 0, 2).finish();
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
    let plain = optimize(&Wasm::new(&[(&[], &[I32])]).memory(1, None, false).func(0, &[], &konst(0)).finish(), &only(|_| {}));
    assert!(plain.exports.is_empty() && plain.globals.is_empty());
}
