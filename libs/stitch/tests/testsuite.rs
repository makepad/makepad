use {
    makepad_stitch::{
        Engine, Error, ExternRef, Func, FuncRef, Global, GlobalType, Instance, Limits, Linker, Mem,
        MemType, Module, Mut, Ref, RefType, Store, Table, TableType, Val, ValType,
    },
    std::{collections::HashMap, sync::Arc},
    wast::{
        core::{HeapType, NanPattern, WastArgCore, WastRetCore},
        parser,
        parser::ParseBuffer,
        QuoteWat, Wast, WastArg, WastDirective, WastExecute, WastInvoke, WastRet, Wat,
    },
};

#[derive(Debug)]
pub(crate) struct WastRunner {
    store: Store,
    linker: Linker,
    instances_by_name: HashMap<String, Instance>,
    current_instance: Option<Instance>,
}

impl WastRunner {
    pub(crate) fn new() -> Self {
        let mut store = Store::new(Engine::new());
        let mut linker = Linker::new();
        let print = Func::wrap(&mut store, || {
            println!("print");
        });
        let print_i32 = Func::wrap(&mut store, |val: i32| {
            println!("{}", val);
        });
        let print_i64 = Func::wrap(&mut store, |val: i64| {
            println!("{}", val);
        });
        let print_f32 = Func::wrap(&mut store, |val: f32| {
            println!("{}", val);
        });
        let print_f64 = Func::wrap(&mut store, |val: f64| {
            println!("{}", val);
        });
        let print_i32_f32 = Func::wrap(&mut store, |val_0: i32, val_1: f32| {
            println!("{} {}", val_0, val_1);
        });
        let print_f64_f64 = Func::wrap(&mut store, |val_0: f64, val_1: f64| {
            println!("{} {}", val_0, val_1);
        });
        let table = Table::new(
            &mut store,
            TableType {
                limits: Limits {
                    min: 10,
                    max: Some(20),
                },
                elem: RefType::FuncRef,
            },
            Ref::null(RefType::FuncRef),
        )
        .unwrap();
        let memory = Mem::new(
            &mut store,
            MemType {
                limits: Limits {
                    min: 1,
                    max: Some(2),
                },
            },
        );
        let global_i32 = Global::new(
            &mut store,
            GlobalType {
                mut_: Mut::Const,
                val: ValType::I32,
            },
            Val::I32(666),
        )
        .unwrap();
        let global_i64 = Global::new(
            &mut store,
            GlobalType {
                mut_: Mut::Const,
                val: ValType::I64,
            },
            Val::I64(666),
        )
        .unwrap();
        let global_f32 = Global::new(
            &mut store,
            GlobalType {
                mut_: Mut::Const,
                val: ValType::F32,
            },
            Val::F32(666.6),
        )
        .unwrap();
        let global_f64 = Global::new(
            &mut store,
            GlobalType {
                mut_: Mut::Const,
                val: ValType::F64,
            },
            Val::F64(666.6),
        )
        .unwrap();
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
        WastRunner {
            store,
            linker,
            instances_by_name: HashMap::new(),
            current_instance: None,
        }
    }

    pub(crate) fn run(&mut self, string: &str) {
        let buf = ParseBuffer::new(string).unwrap();
        let wast = parser::parse::<Wast>(&buf).unwrap();
        for directive in wast.directives {
            match directive {
                WastDirective::Wat(QuoteWat::Wat(Wat::Module(mut module))) => {
                    let name = module.id.map(|id| id.name());
                    let bytes = module.encode().unwrap();
                    self.create_instance(name, &bytes).unwrap();
                }
                WastDirective::Wat(mut wat @ QuoteWat::QuoteModule(_, _)) => {
                    let bytes = wat.encode().unwrap();
                    self.create_instance(None, &bytes).unwrap();
                }
                WastDirective::AssertMalformed {
                    module: QuoteWat::Wat(Wat::Module(mut module)),
                    ..
                } => {
                    let name = module.id.map(|id| id.name());
                    let bytes = module.encode().unwrap();
                    self.create_instance(name, &bytes).unwrap_err();
                }
                WastDirective::AssertInvalid {
                    module: QuoteWat::Wat(Wat::Module(mut module)),
                    ..
                } => {
                    let name = module.id.map(|id| id.name());
                    let bytes = module.encode().unwrap();
                    self.create_instance(name, &bytes).unwrap_err();
                }
                WastDirective::AssertUnlinkable {
                    module: Wat::Module(mut module),
                    ..
                } => {
                    let name = module.id.map(|id| id.name());
                    let bytes = module.encode().unwrap();
                    self.create_instance(name, &bytes).unwrap_err();
                }
                WastDirective::Register { name, module, .. } => {
                    let instance = self
                        .get_instance(module.map(|module| module.name()))
                        .unwrap();
                    self.register(name, instance.clone());
                }
                WastDirective::Invoke(invoke) => {
                    self.invoke(invoke).unwrap();
                }
                WastDirective::AssertTrap { exec, .. } => {
                    self.assert_trap(exec);
                }
                WastDirective::AssertReturn {
                    exec,
                    results: expected_results,
                    ..
                } => {
                    self.assert_return(exec, expected_results).unwrap();
                }
                _ => {}
            }
        }
    }

    fn get_instance(&self, name: Option<&str>) -> Option<&Instance> {
        name.map_or_else(
            || self.current_instance.as_ref(),
            |name| self.instances_by_name.get(name),
        )
    }

    fn create_instance(&mut self, name: Option<&str>, bytes: &[u8]) -> Result<(), Error> {
        let module = Arc::new(Module::new(&self.store.engine(), &bytes)?);
        let instance = self.linker.instantiate(&mut self.store, &module)?;
        if let Some(name) = name {
            self.instances_by_name
                .insert(name.to_string(), instance.clone());
        }
        self.current_instance = Some(instance);
        Ok(())
    }

    fn assert_trap(&mut self, exec: WastExecute<'_>) {
        self.execute(exec).unwrap_err();
    }

    fn assert_return(
        &mut self,
        exec: WastExecute<'_>,
        expected_results: Vec<WastRet<'_>>,
    ) -> Result<(), Error> {
        for (actual_result, expected_result) in
            self.execute(exec)?.into_iter().zip(expected_results)
        {
            assert_result(&self.store, actual_result, expected_result);
        }
        Ok(())
    }

    fn execute(&mut self, exec: WastExecute<'_>) -> Result<Vec<Val>, Error> {
        match exec {
            WastExecute::Invoke(invoke) => self.invoke(invoke),
            WastExecute::Wat(Wat::Module(mut module)) => {
                let name = module.id.map(|id| id.name());
                let bytes = module.encode().unwrap();
                self.create_instance(name, &bytes)?;
                Ok(vec![])
            }
            WastExecute::Get { module, global } => {
                Ok(vec![self.get(module.map(|module| module.name()), global)?])
            }
            _ => unimplemented!(),
        }
    }

    fn invoke(&mut self, invoke: WastInvoke<'_>) -> Result<Vec<Val>, Error> {
        let name = invoke.module.map(|module| module.name());
        let instance = self.get_instance(name).unwrap();
        let func = instance.export(invoke.name).unwrap().to_func().unwrap();
        let args: Vec<Val> = invoke
            .args
            .into_iter()
            .map(|arg| match arg {
                WastArg::Core(arg) => match arg {
                    WastArgCore::I32(arg) => arg.into(),
                    WastArgCore::I64(arg) => arg.into(),
                    WastArgCore::F32(arg) => f32::from_bits(arg.bits).into(),
                    WastArgCore::F64(arg) => f64::from_bits(arg.bits).into(),
                    WastArgCore::RefNull(HeapType::Func) => FuncRef::null().into(),
                    WastArgCore::RefNull(HeapType::Extern) => ExternRef::null().into(),
                    WastArgCore::RefExtern(val) => ExternRef::new(&mut self.store, val).into(),
                    _ => unimplemented!(),
                },
                _ => unimplemented!(),
            })
            .collect();
        let mut results: Vec<_> = func
            .type_(&mut self.store)
            .results()
            .iter()
            .copied()
            .map(|type_| Val::default(type_))
            .collect();
        func.call(&mut self.store, &args, &mut results)?;
        Ok(results)
    }

    fn get(&mut self, module_name: Option<&str>, global_name: &str) -> Result<Val, Error> {
        let instance = self.get_instance(module_name).unwrap();
        let global = instance.exported_global(global_name).unwrap();
        Ok(global.get(&mut self.store))
    }

    fn register(&mut self, name: &str, instance: Instance) {
        for (export_name, export_val) in instance.exports() {
            self.linker.define(name, export_name, export_val);
        }
        self.current_instance = Some(instance);
    }
}

fn assert_result(store: &Store, actual: Val, expected: WastRet<'_>) {
    match expected {
        WastRet::Core(expected) => match expected {
            WastRetCore::I32(expected) => {
                assert_eq!(actual.to_i32().unwrap(), expected)
            }
            WastRetCore::I64(expected) => {
                assert_eq!(actual.to_i64().unwrap(), expected)
            }
            WastRetCore::F32(expected) => match expected {
                NanPattern::CanonicalNan => {
                    assert!(
                        actual.to_f32().unwrap().to_bits() & 0b0_11111111_11111111111111111111111
                            == 0b0_11111111_10000000000000000000000
                    );
                }
                NanPattern::ArithmeticNan => {
                    assert!(
                        actual.to_f32().unwrap().to_bits() & 0b0_11111111_11111111111111111111111
                            > 0b0_11111111_10000000000000000000000
                    );
                }
                NanPattern::Value(expected_result) => {
                    assert_eq!(actual.to_f32().unwrap().to_bits(), expected_result.bits)
                }
            },
            WastRetCore::F64(expected) => match expected {
                NanPattern::CanonicalNan => {
                    assert!(
                        actual.to_f64().unwrap().to_bits()
                            & 0b0_11111111111_1111111111111111111111111111111111111111111111111111
                            == 0b0_11111111111_1000000000000000000000000000000000000000000000000000
                    );
                }
                NanPattern::ArithmeticNan => {
                    assert!(
                        actual.to_f64().unwrap().to_bits()
                            & 0b0_11111111111_1111111111111111111111111111111111111111111111111111
                            > 0b0_11111111111_1000000000000000000000000000000000000000000000000000
                    );
                }
                NanPattern::Value(expected_result) => {
                    assert_eq!(actual.to_f64().unwrap().to_bits(), expected_result.bits)
                }
            },
            WastRetCore::RefNull(Some(HeapType::Func)) => {
                assert_eq!(actual, Val::FuncRef(FuncRef::null()));
            }
            WastRetCore::RefNull(Some(HeapType::Extern)) => {
                assert_eq!(actual, Val::ExternRef(ExternRef::null()));
            }
            WastRetCore::RefExtern(expected) => {
                assert_eq!(
                    actual
                        .to_extern_ref()
                        .unwrap()
                        .get(store)
                        .map(|val| { *val.downcast_ref::<u32>().unwrap() }),
                    expected
                );
            }
            _ => unimplemented!(),
        },
        _ => unimplemented!(),
    }
}

macro_rules! testsuite {
    ($($name:ident => $file_name:literal,)*) => {
        $(
            #[test]
            fn $name() {
                use std::{fs, path::PathBuf};

                let mut runner = WastRunner::new();
                let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
                path.push("tests/testsuite");
                path.push($file_name);
                let string = fs::read_to_string(path).unwrap();
                runner.run(&string);
            }
        )*
    }
}

testsuite! {
    address => "address.wast",
    align => "align.wast",
    binary_leb128 => "binary-leb128.wast",
    binary => "binary.wast",
    block => "block.wast",
    br => "br.wast",
    br_if => "br_if.wast",
    br_table => "br_table.wast",
    bulk => "bulk.wast",
    call => "call.wast",
    call_indirect => "call_indirect.wast",
    comments => "comments.wast",
    r#const => "const.wast",
    conversions => "conversions.wast",
    custom => "custom.wast",
    data => "data.wast",
    elem => "elem.wast",
    endianness => "endianness.wast",
    exports => "exports.wast",
    f32 => "f32.wast",
    f32_bitwise => "f32_bitwise.wast",
    f32_cmp => "f32_cmp.wast",
    f64 => "f64.wast",
    f64_bitwise => "f64_bitwise.wast",
    f64_cmp => "f64_cmp.wast",
    fac => "fac.wast",
    float_exprs => "float_exprs.wast",
    float_literals => "float_literals.wast",
    float_memory => "float_memory.wast",
    float_misc => "float_misc.wast",
    forward => "forward.wast",
    func => "func.wast",
    func_ptrs => "func_ptrs.wast",
    global => "global.wast",
    i32 => "i32.wast",
    i64 => "i64.wast",
    r#if => "if.wast",
    imports => "imports.wast",
    inline_module => "inline-module.wast",
    int_exprs => "int_exprs.wast",
    int_literals => "int_literals.wast",
    labels => "labels.wast",
    left_to_right => "left-to-right.wast",
    linking => "linking.wast",
    load => "load.wast",
    local_get => "local_get.wast",
    local_set => "local_set.wast",
    local_tee => "local_tee.wast",
    r#loop => "loop.wast",
    memory => "memory.wast",
    memory_copy => "memory_copy.wast",
    memory_fill => "memory_fill.wast",
    memory_grow => "memory_grow.wast",
    memory_init => "memory_init.wast",
    memory_redundancy => "memory_redundancy.wast",
    memory_size => "memory_size.wast",
    memory_trap => "memory_trap.wast",
    // names => "names.wast",
    nop => "nop.wast",
    obsolete_keywords => "obsolete-keywords.wast",
    ref_func => "ref_func.wast",
    ref_is_null => "ref_is_null.wast",
    ref_null => "ref_null.wast",
    r#return => "return.wast",
    select => "select.wast",
    /*
    simd_address => "simd/address.wast",
    simd_align => "simd/align.wast",
    simd_bit_shift => "simd/bit_shift.wast",
    simd_bitwise => "simd/bitwise.wast",
    simd_boolean => "simd/boolean.wast",
    simd_const => "simd/const.wast",
    simd_conversions => "simd/conversions.wast",
    simd_f32x4 => "simd/f32x4.wast",
    simd_f32x4_arith => "simd/f32x4-arith.wast",
    simd_f32x4_cmp => "simd/f32x4-cmp.wast",
    simd_f32x4_pmin_pmax => "simd/f32x4-pmin-pmax.wast",
    simd_f32x4_rounding => "simd/f32x4-rounding.wast",
    simd_f64x2 => "simd/f64x2.wast",
    simd_f64x2_arith => "simd/f64x2-arith.wast",
    simd_f64x2_cmp => "simd/f64x2-cmp.wast",
    simd_f64x2_pmin_pmax => "simd/f64x2-pmin-pmax.wast",
    simd_f64x2_rounding => "simd/f64x2-rounding.wast",
    simd_i16x8_arith => "simd/i16x8-arith.wast",
    simd_i16x8_arith2 => "simd/i16x8-arith2.wast",
    simd_i16x8_cmp => "simd/i16x8-cmp.wast",
    simd_i16x8_extadd_pairwise_i8x16 => "simd/i16x8-extadd-pairwise-i8x16.wast",
    simd_i16x8_extmul_i8x16 => "simd/i16x8-extmul-i8x16.wast",
    simd_i16x8_q15mulr_sat_s => "simd/i16x8-q15mulr-sat_s.wast",
    simd_i16x8_sat_arith => "simd/i16x8-sat-arith.wast",
    simd_i32x4_arith => "simd/i32x4-arith.wast",
    simd_i32x4_arith2 => "simd/i32x4-arith2.wast",
    simd_i32x4_cmp => "simd/i32x4-cmp.wast",
    simd_i32x4_dot_i16x8 => "simd/i32x4-dot_i16x8.wast",
    simd_i32x4_extadd_pairwise_i16x8 => "simd/i32x4-extadd-pairwise-i16x8.wast",
    simd_i32x4_extmul_i16x8 => "simd/i32x4-extmul-i16x8.wast",
    simd_i32x4_trunc_sat_f32x4 => "simd/i32x4-trunc-sat-f32x4.wast",
    simd_i32x4_trunc_sat_f64x2 => "simd/i32x4-trunc-sat-f64x2.wast",
    simd_i64x2_arith => "simd/i64x2-arith.wast",
    simd_i64x2_arith2 => "simd/i64x2-arith2.wast",
    simd_i64x2_cmp => "simd/i64x2-cmp.wast",
    simd_i64x2_extmul_i32x4 => "simd/i64x2-extmul-i32x4.wast",
    simd_i8x16_arith => "simd/i8x16-arith.wast",
    simd_i8x16_arith2 => "simd/i8x16-arith2.wast",
    simd_i8x16_cmp => "simd/i8x16-cmp.wast",
    simd_i8x16_sat_arith => "simd/i8x16-sat-arith.wast",
    simd_int_to_int_extend => "simd/int-to-int-extend.wast",
    simd_lane => "simd/lane.wast",
    simd_linking => "simd/linking.wast",
    simd_load => "simd/load.wast",
    simd_load16_lane => "simd/load16_lane.wast",
    simd_load32_lane => "simd/load32_lane.wast",
    simd_load64_lane => "simd/load64_lane.wast",
    simd_load8_lane => "simd/load8_lane.wast",
    simd_load_extend => "simd/load_extend.wast",
    simd_load_splat => "simd/load_splat.wast",
    simd_load_zero => "simd/load_zero.wast",
    simd_splat => "simd/splat.wast",
    simd_store => "simd/store.wast",
    simd_store16_lane => "simd/store16_lane.wast",
    simd_store32_lane => "simd/store32_lane.wast",
    simd_store64_lane => "simd/store64_lane.wast",
    simd_store8_lane => "simd/store8_lane.wast",
    */
    skip_stack_guard_page => "skip-stack-guard-page.wast",
    stack => "stack.wast",
    start => "start.wast",
    store => "store.wast",
    switch => "switch.wast",
    table_sub => "table-sub.wast",
    table => "table.wast",
    table_copy => "table_copy.wast",
    table_fill => "table_fill.wast",
    table_get => "table_get.wast",
    table_grow => "table_grow.wast",
    table_init => "table_init.wast",
    table_set => "table_set.wast",
    table_size => "table_size.wast",
    token => "token.wast",
    traps => "traps.wast",
    r#type => "type.wast",
    unreachable => "unreachable.wast",
    unreached_invalid => "unreached-invalid.wast",
    unreached_valid => "unreached-valid.wast",
    unwind => "unwind.wast",
    utf8_custom_section_id => "utf8-custom-section-id.wast",
    utf8_import_field => "utf8-import-field.wast",
    utf8_import_module => "utf8-import-module.wast",
    utf8_invalid_encoding => "utf8-invalid-encoding.wast",
}
