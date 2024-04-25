#![no_main]

use {
    libfuzzer_sys::{arbitrary::Unstructured, fuzz_target},
    wasm_smith::{Config, Module as WasmSmithModule},
};

fuzz_target!(|bytes: &[u8]| {
    let mut unstructured = Unstructured::new(&bytes);
    let module = WasmSmithModule::new(Config {
        max_memory32_pages: 16384,
        ..Config::default()
    }, &mut unstructured).unwrap();
    println!("{:#?}", module);
    let bytes = module.to_bytes();
    let stitch_engine = stitch::Engine::new();
    let mut stitch_store = stitch::Store::new(stitch_engine);
    let Ok(stitch_module) = stitch::Module::new(stitch_store.engine(), &bytes) else {
        return
    };
    if stitch_module.imports().count() > 0 {
        return;
    }
    let Ok(stitch_instance) = stitch_module.instantiate(&mut stitch_store, &[]) else {
        return;
    };
    let wasmtime_config = wasmtime::Config::new();
    let wasmtime_engine = wasmtime::Engine::new(&wasmtime_config).unwrap();
    let mut wasmtime_store = wasmtime::Store::new(&wasmtime_engine, ());
    let Ok(wasmtime_module) = wasmtime::Module::new(wasmtime_store.engine(), &bytes) else {
        return
    };
    let mut wasmtime_linker = wasmtime::Linker::new(wasmtime_store.engine());
    let Ok(wasmtime_instance) = wasmtime_linker.instantiate(&mut wasmtime_store, &wasmtime_module) else {
        return;
    };
    for (name, _) in stitch_instance.exports() {
        match stitch_instance.export(name).unwrap() {
            stitch::ExternVal::Func(stitch_func) => {
                let wasmtime_func = wasmtime_instance.get_func(&mut wasmtime_store, name).unwrap();

                let stitch_args = stitch_func.type_(&stitch_store).param_types().iter().map(|&param| arbitrary_stitch_val(param, &mut unstructured)).collect::<Vec<_>>();
                let mut stitch_results = stitch_func.type_(&stitch_store).result_types().iter().map(|&result| stitch::Val::new(result)).collect::<Vec<_>>();
                stitch_func.call(&mut stitch_store, &stitch_args, &mut stitch_results);

                let wasmtime_args = wasmtime_func.ty(&wasmtime_store).params().map(|param| arbitrary_wasmtime_val(param, &mut unstructured)).collect::<Vec<_>>();
                let mut wasmtime_results = wasmtime_func.ty(&wasmtime_store).results().map(|result| match result {
                    wasmtime::ValType::I32 => wasmtime::Val::I32(0),
                    wasmtime::ValType::I64 => wasmtime::Val::I64(0),
                    wasmtime::ValType::F32 => wasmtime::Val::F32(0),
                    wasmtime::ValType::F64 => wasmtime::Val::F64(0),
                    _ => unimplemented!()
                }).collect::<Vec<_>>();
                wasmtime_func.call(&mut wasmtime_store, &wasmtime_args, &mut wasmtime_results);

                for (stitch_result, wasmtime_result) in stitch_results.iter().zip(wasmtime_results.iter()) {
                    match (stitch_result, wasmtime_result) {
                        (stitch::Val::I32(stitch_val), wasmtime::Val::I32(wasmtime_val)) => assert_eq!(*stitch_val, *wasmtime_val),
                        (stitch::Val::I64(stitch_val), wasmtime::Val::I64(wasmtime_val)) => assert_eq!(*stitch_val, *wasmtime_val),
                        (stitch::Val::F32(stitch_val), wasmtime::Val::F32(wasmtime_val)) => assert_eq!(stitch_val.to_bits(), *wasmtime_val),
                        (stitch::Val::F64(stitch_val), wasmtime::Val::F64(wasmtime_val)) => assert_eq!(stitch_val.to_bits(), *wasmtime_val),
                        _ => unimplemented!()
                    }
                }
            }
            _ => {}
        }
    }
});

fn arbitrary_stitch_val(type_: stitch::ValType, unstructured: &mut Unstructured) -> stitch::Val {
    match type_ {
        stitch::ValType::I32 => stitch::Val::I32(unstructured.arbitrary().unwrap()),
        stitch::ValType::I64 => stitch::Val::I64(unstructured.arbitrary().unwrap()),
        stitch::ValType::F32 => stitch::Val::F32(unstructured.arbitrary().unwrap()),
        stitch::ValType::F64 => stitch::Val::F64(unstructured.arbitrary().unwrap()),
        _ => unimplemented!()
    }
}

fn arbitrary_wasmtime_val(type_: wasmtime::ValType, unstructured: &mut Unstructured) -> wasmtime::Val {
    match type_ {
        wasmtime::ValType::I32 => wasmtime::Val::I32(unstructured.arbitrary().unwrap()),
        wasmtime::ValType::I64 => wasmtime::Val::I64(unstructured.arbitrary().unwrap()),
        wasmtime::ValType::F32 => wasmtime::Val::F32(unstructured.arbitrary().unwrap()),
        wasmtime::ValType::F64 => wasmtime::Val::F64(unstructured.arbitrary().unwrap()),
        _ => unimplemented!()
    }
}