#![no_main]

use {
    libfuzzer_sys::{arbitrary::Unstructured, fuzz_target},
    wasm_smith::{Config, Module as WasmSmithModule},
};

fuzz_target!(|bytes: &[u8]| {
    let mut unstructured = Unstructured::new(bytes);
    let module = WasmSmithModule::new(
        Config {
            max_memory32_pages: 16384,
            ..Config::default()
        },
        &mut unstructured,
    )
    .unwrap();
    let bytes = module.to_bytes();

    let stitch_engine = makepad_stitch::Engine::new();
    let mut stitch_store = makepad_stitch::Store::new(stitch_engine);
    let Ok(stitch_module) = makepad_stitch::Module::new(stitch_store.engine(), &bytes) else {
        return;
    };
    if stitch_module.imports().count() > 0 {
        return;
    }

    let linker = makepad_stitch::Linker::new();
    let Ok(stitch_instance) = linker.instantiate(&mut stitch_store, &stitch_module) else {
        return;
    };

    for (name, _) in stitch_instance.exports() {
        if let makepad_stitch::ExternVal::Func(stitch_func) =
            stitch_instance.exported_val(name).unwrap()
        {
            let stitch_args = stitch_func
                .type_(&stitch_store)
                .params()
                .iter()
                .map(|&param| arbitrary_stitch_val(param, &mut unstructured))
                .collect::<Vec<_>>();
            let mut stitch_results = stitch_func
                .type_(&stitch_store)
                .results()
                .iter()
                .map(|&result| makepad_stitch::Val::default(result))
                .collect::<Vec<_>>();

            let _ = stitch_func.call(&mut stitch_store, &stitch_args, &mut stitch_results);
        }
    }
});

fn arbitrary_stitch_val(
    type_: makepad_stitch::ValType,
    unstructured: &mut Unstructured,
) -> makepad_stitch::Val {
    match type_ {
        makepad_stitch::ValType::I32 => makepad_stitch::Val::I32(unstructured.arbitrary().unwrap()),
        makepad_stitch::ValType::I64 => makepad_stitch::Val::I64(unstructured.arbitrary().unwrap()),
        makepad_stitch::ValType::F32 => makepad_stitch::Val::F32(unstructured.arbitrary().unwrap()),
        makepad_stitch::ValType::F64 => makepad_stitch::Val::F64(unstructured.arbitrary().unwrap()),
        _ => unimplemented!(),
    }
}
