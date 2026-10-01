//! The core spec testsuite (kept in libs/stitch) against the optimiser:
//! - the validator accepts every valid module and refuses every invalid or
//!   malformed binary one, for the core suite and the proposals it covers;
//! - every module of the suites stitch runs is optimised with all passes and
//!   must still satisfy all of that suite's assertions under stitch.

use makepad_stitch::{
    Engine, Error, ExternRef, Func, FuncRef, Global, GlobalType, Instance, Limits, Linker, Mem,
    MemType, Module, Mut, Ref, RefType, Store, Table, TableType, Val, ValType, V128,
};
use makepad_wasm_strip::{wasm_optimize_checked, wasm_validate, OptimizeOptions};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use wast::{
    core::{HeapType, NanPattern, V128Pattern, WastArgCore, WastRetCore},
    parser::{self, ParseBuffer},
    QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
};

fn testsuite_dir() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../stitch/tests/testsuite");
    path
}

fn wast_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().map_or(false, |ext| ext == "wast"))
        .collect();
    out.sort();
    out
}

/// Features the decoder deliberately refuses; modules using them are not
/// held against the validator.
fn unsupported(msg: &str) -> bool {
    msg.contains("unsupported") || msg.contains("unknown value type") || msg.contains("unknown opcode")
}

const SUPERSEDED: &[&str] = &["multiple memories", "multiple tables", "zero byte expected"];

/// Validator conformance for one file: returns the mismatches.
fn conformance(path: &PathBuf) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap();
    let buf = match ParseBuffer::new(&text) {
        Ok(buf) => buf,
        Err(_) => return vec![],
    };
    let Ok(wast) = parser::parse::<Wast>(&buf) else {
        return vec![];
    };
    let name = path.file_name().unwrap().to_string_lossy().to_string();
    let mut problems = Vec::new();
    for directive in wast.directives {
        let (mut module, expect_valid) = match directive {
            WastDirective::Wat(module) => (module, true),
            WastDirective::AssertInvalid { module, message, .. }
            | WastDirective::AssertMalformed {
                module: module @ QuoteWat::Wat(_),
                message,
                ..
            } => {
                // Later proposals (which the validator implements) made
                // these valid: more than one memory or table, and a memory
                // index LEB where MVP had a zero byte.
                if SUPERSEDED.contains(&message) {
                    continue;
                }
                (module, false)
            }
            _ => continue,
        };
        let Ok(bytes) = module.encode() else {
            continue;
        };
        match (wasm_validate(&bytes), expect_valid) {
            (Ok(()), true) | (Err(_), false) => {}
            (Err(msg), true) => {
                if !unsupported(&msg) {
                    problems.push(format!("{name}: valid module refused: {msg}"));
                }
            }
            (Ok(()), false) => problems.push(format!(
                "{name}: invalid module accepted (line {})",
                text[..module_offset(&module).min(text.len())].lines().count()
            )),
        }
    }
    problems
}

fn module_offset(module: &QuoteWat) -> usize {
    match module {
        QuoteWat::Wat(Wat::Module(m)) => m.span.offset(),
        QuoteWat::Wat(Wat::Component(c)) => c.span.offset(),
        QuoteWat::QuoteModule(span, _) | QuoteWat::QuoteComponent(span, _) => span.offset(),
    }
}

#[test]
fn validator_conformance() {
    let root = testsuite_dir();
    let mut files = wast_files(&root);
    for proposal in ["extended-const", "multi-memory", "relaxed-simd", "tail-call", "threads"] {
        files.extend(wast_files(&root.join("proposals").join(proposal)));
    }
    let mut problems = Vec::new();
    for file in &files {
        problems.extend(conformance(file));
    }
    for problem in &problems {
        eprintln!("{problem}");
    }
    assert!(problems.is_empty(), "{} validator mismatches", problems.len());
    assert!(files.len() > 150);
}

// --- Semantics: optimised modules under stitch ---------------------------

struct Runner {
    store: Store,
    linker: Linker,
    instances: HashMap<String, Instance>,
    current: Option<Instance>,
    optimized: usize,
}

fn optimize(bytes: &[u8]) -> Vec<u8> {
    match wasm_optimize_checked(bytes, &OptimizeOptions::default()) {
        Ok((out, report)) => {
            for pass in &report.passes {
                if let Some(reason) = &pass.reverted {
                    panic!("pass {} reverted: {reason}", pass.name);
                }
            }
            out
        }
        // Inputs the optimiser does not take are run as they are.
        Err(msg) => {
            assert!(
                unsupported(&msg) || Module::new(&Engine::new(), bytes).is_err(),
                "optimiser refused a module stitch accepts: {msg}"
            );
            bytes.to_vec()
        }
    }
}

impl Runner {
    fn new() -> Runner {
        let mut store = Store::new(Engine::new());
        let mut linker = Linker::new();
        let print = Func::wrap(&mut store, || {});
        let print_i32 = Func::wrap(&mut store, |_: i32| {});
        let print_i64 = Func::wrap(&mut store, |_: i64| {});
        let print_f32 = Func::wrap(&mut store, |_: f32| {});
        let print_f64 = Func::wrap(&mut store, |_: f64| {});
        let print_i32_f32 = Func::wrap(&mut store, |_: i32, _: f32| {});
        let print_f64_f64 = Func::wrap(&mut store, |_: f64, _: f64| {});
        let table = Table::new(
            &mut store,
            TableType {
                limits: Limits { min: 10, max: Some(20) },
                elem: RefType::FuncRef,
            },
            Ref::null(RefType::FuncRef),
        )
        .unwrap();
        let memory = Mem::new(
            &mut store,
            MemType {
                limits: Limits { min: 1, max: Some(2) },
            },
        );
        let global = |store: &mut Store, val: ValType, v: Val| {
            Global::new(store, GlobalType { mut_: Mut::Const, val }, v).unwrap()
        };
        let global_i32 = global(&mut store, ValType::I32, Val::I32(666));
        let global_i64 = global(&mut store, ValType::I64, Val::I64(666));
        let global_f32 = global(&mut store, ValType::F32, Val::F32(666.6));
        let global_f64 = global(&mut store, ValType::F64, Val::F64(666.6));
        linker.define("spectest", "print", print);
        linker.define("spectest", "print_i32", print_i32);
        linker.define("spectest", "print_i64", print_i64);
        linker.define("spectest", "print_f32", print_f32);
        linker.define("spectest", "print_f64", print_f64);
        linker.define("spectest", "print_i32_f32", print_i32_f32);
        linker.define("spectest", "print_f64_f64", print_f64_f64);
        linker.define("spectest", "table", table);
        linker.define("spectest", "memory", memory);
        linker.define("spectest", "global_i32", global_i32);
        linker.define("spectest", "global_i64", global_i64);
        linker.define("spectest", "global_f32", global_f32);
        linker.define("spectest", "global_f64", global_f64);
        Runner {
            store,
            linker,
            instances: HashMap::new(),
            current: None,
            optimized: 0,
        }
    }

    fn run(&mut self, text: &str) {
        let buf = ParseBuffer::new(text).unwrap();
        let wast = parser::parse::<Wast>(&buf).unwrap();
        for directive in wast.directives {
            match directive {
                WastDirective::Wat(QuoteWat::Wat(Wat::Module(mut module))) => {
                    let name = module.id.map(|id| id.name());
                    let bytes = module.encode().unwrap();
                    self.instantiate(name, &bytes, true).unwrap();
                }
                WastDirective::Wat(mut wat @ QuoteWat::QuoteModule(_, _)) => {
                    let bytes = wat.encode().unwrap();
                    self.instantiate(None, &bytes, true).unwrap();
                }
                WastDirective::Register { name, module, .. } => {
                    let instance = self.instance(module.map(|m| m.name())).clone();
                    for (export_name, export_val) in instance.exports() {
                        self.linker.define(name, export_name, export_val);
                    }
                    self.current = Some(instance);
                }
                WastDirective::Invoke(invoke) => {
                    self.invoke(invoke).unwrap();
                }
                WastDirective::AssertTrap { exec, .. } => {
                    assert!(self.execute(exec).is_err(), "expected a trap");
                }
                WastDirective::AssertReturn { exec, results, .. } => {
                    let actual = self.execute(exec).unwrap();
                    for (actual, expected) in actual.into_iter().zip(results) {
                        assert_result(&self.store, actual, expected);
                    }
                }
                _ => {}
            }
        }
    }

    fn instance(&self, name: Option<&str>) -> &Instance {
        match name {
            Some(name) => &self.instances[name],
            None => self.current.as_ref().unwrap(),
        }
    }

    fn instantiate(&mut self, name: Option<&str>, bytes: &[u8], opt: bool) -> Result<(), Error> {
        let bytes = if opt {
            self.optimized += 1;
            optimize(bytes)
        } else {
            bytes.to_vec()
        };
        let module = Arc::new(Module::new(self.store.engine(), &bytes).expect("optimised module loads"));
        let instance = self.linker.instantiate(&mut self.store, &module)?;
        if let Some(name) = name {
            self.instances.insert(name.to_string(), instance.clone());
        }
        self.current = Some(instance);
        Ok(())
    }

    fn execute(&mut self, exec: WastExecute<'_>) -> Result<Vec<Val>, Error> {
        match exec {
            WastExecute::Invoke(invoke) => self.invoke(invoke),
            WastExecute::Wat(Wat::Module(mut module)) => {
                let name = module.id.map(|id| id.name());
                let bytes = module.encode().unwrap();
                self.instantiate(name, &bytes, true)?;
                Ok(vec![])
            }
            WastExecute::Get { module, global } => {
                let instance = self.instance(module.map(|m| m.name()));
                let global = instance.exported_global(global).unwrap();
                Ok(vec![global.get(&self.store)])
            }
            _ => unimplemented!(),
        }
    }

    fn invoke(&mut self, invoke: WastInvoke<'_>) -> Result<Vec<Val>, Error> {
        let func = self
            .instance(invoke.module.map(|m| m.name()))
            .exported_func(invoke.name)
            .unwrap();
        let args: Vec<Val> = invoke
            .args
            .into_iter()
            .map(|arg| match arg {
                WastArg::Core(WastArgCore::I32(v)) => v.into(),
                WastArg::Core(WastArgCore::I64(v)) => v.into(),
                WastArg::Core(WastArgCore::F32(v)) => f32::from_bits(v.bits).into(),
                WastArg::Core(WastArgCore::F64(v)) => f64::from_bits(v.bits).into(),
                WastArg::Core(WastArgCore::V128(v)) => V128::from_bytes(v.to_le_bytes()).into(),
                WastArg::Core(WastArgCore::RefNull(HeapType::Func)) => FuncRef::null().into(),
                WastArg::Core(WastArgCore::RefNull(HeapType::Extern)) => ExternRef::null().into(),
                WastArg::Core(WastArgCore::RefExtern(v)) => ExternRef::new(&mut self.store, v).into(),
                _ => unimplemented!(),
            })
            .collect();
        let mut results: Vec<Val> = func
            .type_(&self.store)
            .results()
            .iter()
            .map(|ty| Val::default(*ty))
            .collect();
        func.call(&mut self.store, &args, &mut results)?;
        Ok(results)
    }
}

fn nan32(bits: u32, canonical: bool) -> bool {
    let payload = bits & 0x7fff_ffff;
    if canonical {
        payload == 0x7fc0_0000
    } else {
        payload >= 0x7fc0_0000
    }
}

fn nan64(bits: u64, canonical: bool) -> bool {
    let payload = bits & 0x7fff_ffff_ffff_ffff;
    if canonical {
        payload == 0x7ff8_0000_0000_0000
    } else {
        payload >= 0x7ff8_0000_0000_0000
    }
}

fn check_f32(bits: u32, expected: &NanPattern<wast::token::Float32>) {
    match expected {
        NanPattern::CanonicalNan => assert!(nan32(bits, true)),
        NanPattern::ArithmeticNan => assert!(nan32(bits, false)),
        NanPattern::Value(v) => assert_eq!(bits, v.bits),
    }
}

fn check_f64(bits: u64, expected: &NanPattern<wast::token::Float64>) {
    match expected {
        NanPattern::CanonicalNan => assert!(nan64(bits, true)),
        NanPattern::ArithmeticNan => assert!(nan64(bits, false)),
        NanPattern::Value(v) => assert_eq!(bits, v.bits),
    }
}

fn assert_result(store: &Store, actual: Val, expected: WastRet<'_>) {
    let WastRet::Core(expected) = expected else {
        unimplemented!()
    };
    match expected {
        WastRetCore::I32(v) => assert_eq!(actual.to_i32().unwrap(), v),
        WastRetCore::I64(v) => assert_eq!(actual.to_i64().unwrap(), v),
        WastRetCore::F32(p) => check_f32(actual.to_f32().unwrap().to_bits(), &p),
        WastRetCore::F64(p) => check_f64(actual.to_f64().unwrap().to_bits(), &p),
        WastRetCore::V128(pattern) => {
            let bytes = actual.to_v128().unwrap().to_bytes();
            let lane = |i: usize, w: usize| -> u64 {
                let mut out = [0u8; 8];
                out[..w].copy_from_slice(&bytes[i * w..i * w + w]);
                u64::from_le_bytes(out)
            };
            match pattern {
                V128Pattern::I8x16(v) => v.iter().enumerate().for_each(|(i, e)| assert_eq!(lane(i, 1) as u8 as i8, *e)),
                V128Pattern::I16x8(v) => v.iter().enumerate().for_each(|(i, e)| assert_eq!(lane(i, 2) as u16 as i16, *e)),
                V128Pattern::I32x4(v) => v.iter().enumerate().for_each(|(i, e)| assert_eq!(lane(i, 4) as u32 as i32, *e)),
                V128Pattern::I64x2(v) => v.iter().enumerate().for_each(|(i, e)| assert_eq!(lane(i, 8) as i64, *e)),
                V128Pattern::F32x4(v) => v.iter().enumerate().for_each(|(i, e)| check_f32(lane(i, 4) as u32, e)),
                V128Pattern::F64x2(v) => v.iter().enumerate().for_each(|(i, e)| check_f64(lane(i, 8), e)),
            }
        }
        WastRetCore::RefNull(Some(HeapType::Func)) => assert_eq!(actual, Val::FuncRef(FuncRef::null())),
        WastRetCore::RefNull(Some(HeapType::Extern)) => {
            assert_eq!(actual, Val::ExternRef(ExternRef::null()))
        }
        WastRetCore::RefExtern(expected) => assert_eq!(
            actual
                .to_extern_ref()
                .unwrap()
                .get(store)
                .map(|val| *val.downcast_ref::<u32>().unwrap()),
            expected
        ),
        _ => unimplemented!(),
    }
}

/// The suites stitch runs (see libs/stitch/tests/testsuite.rs).
const STITCH_SUITES: &[&str] = &[
    "address", "align", "binary-leb128", "binary", "block", "br", "br_if", "br_table", "bulk",
    "call", "call_indirect", "comments", "const", "conversions", "custom", "data", "elem",
    "endianness", "exports", "f32", "f32_bitwise", "f32_cmp", "f64", "f64_bitwise", "f64_cmp",
    "fac", "float_exprs", "float_literals", "float_memory", "float_misc", "forward", "func",
    "func_ptrs", "global", "i32", "i64", "if", "imports", "inline-module", "int_exprs",
    "int_literals", "labels", "left-to-right", "linking", "load", "local_get", "local_set",
    "local_tee", "loop", "memory", "memory_copy", "memory_fill", "memory_grow", "memory_init",
    "memory_redundancy", "memory_size", "memory_trap", "nop", "obsolete-keywords", "ref_func",
    "ref_is_null", "ref_null", "return", "select", "skip-stack-guard-page", "stack", "start",
    "store", "switch", "table-sub", "table", "table_copy", "table_fill", "table_get",
    // table_grow: stitch panics compiling `local.tee` of a funcref followed
    // by `ref.is_null` (compile.rs visit_ref_is_null), which peephole makes
    // of its `local.set`/`local.get` pair; valid wasm, a stitch bug.
    "table_init", "table_set", "table_size", "token", "traps", "type",
    "unreachable", "unreached-invalid", "unreached-valid", "unwind", "utf8-custom-section-id",
    "utf8-import-field", "utf8-import-module", "utf8-invalid-encoding", "simd_address",
    "simd_bitwise", "simd_f32x4", "simd_f32x4_arith", "simd_f32x4_cmp", "simd_f32x4_pmin_pmax",
    "simd_f32x4_rounding", "simd_store",
];

#[test]
fn optimised_modules_pass_the_spec_suite() {
    let mut total = 0;
    for suite in STITCH_SUITES {
        let path = testsuite_dir().join(format!("{suite}.wast"));
        let text = std::fs::read_to_string(&path).unwrap();
        let mut runner = Runner::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runner.run(&text)));
        if result.is_err() {
            panic!("{suite}.wast fails after optimising");
        }
        total += runner.optimized;
    }
    assert!(total > 500, "only {total} modules optimised");
}

