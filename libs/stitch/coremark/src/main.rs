fn clock_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn stitch(bytes: &[u8]) -> f32 {
    use makepad_stitch::*;

    let engine = Engine::new();
    let mut store = Store::new(engine);
    let module = Module::new(store.engine(), bytes).unwrap();
    let mut linker = Linker::new();
    let clock_ms = Func::wrap(&mut store, clock_ms);
    linker.define("env", "clock_ms", clock_ms);
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let run = instance.exported_func("run").unwrap();
    let mut results = [Val::F32(0.0)];
    run.call(&mut store, &[], &mut results).unwrap();
    results[0].to_f32().unwrap()
}

fn wasm3(bytes: &[u8]) -> f32 {
    use wasm3::Environment;

    wasm3::make_func_wrapper!(clock_ms_wrap: clock_ms() -> u64);

    let environment = Environment::new().unwrap();
    let runtime = environment.create_runtime(1024 * 1024).unwrap();
    let mut module = runtime.parse_and_load_module(bytes).unwrap();
    module
        .link_function::<(), u64>("env", "clock_ms", clock_ms_wrap)
        .unwrap();
    let run = module.find_function::<(), f32>("run").unwrap();
    run.call().unwrap()
}

fn wasmi(bytes: &[u8]) -> f32 {
    use wasmi::{core::F32, *};

    let config = Config::default();
    let engine = Engine::new(&config);
    let mut store = Store::new(&engine, ());
    let module = Module::new(&engine, bytes).unwrap();
    let mut linker = Linker::new(&engine);
    let clock_ms = Func::wrap(&mut store, clock_ms);
    linker.define("env", "clock_ms", clock_ms).unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let instance = instance.start(&mut store).unwrap();
    let run = instance.get_func(&store, "run").unwrap();
    let mut results = [Value::F32(F32::from_float(0.0))];
    run.call(&mut store, &[], &mut results).unwrap();
    results[0].f32().unwrap().to_float()
}

fn wasmtime(bytes: &[u8]) -> f32 {
    use wasmtime::*;

    let config = Config::default();
    let engine = Engine::new(&config).unwrap();
    let mut store = Store::new(&engine, ());
    let module = Module::new(&engine, bytes).unwrap();
    let mut linker = Linker::new(&engine);
    let clock_ms = Func::wrap(&mut store, clock_ms);
    linker
        .define(&mut store, "env", "clock_ms", clock_ms)
        .unwrap();
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let run = instance.get_func(&mut store, "run").unwrap();
    let mut results = [Val::F32(0)];
    run.call(&mut store, &[], &mut results).unwrap();
    results[0].f32().unwrap()
}

fn main() {
    let bytes = include_bytes!("coremark-minimal.wasm");
    println!("stitch {}", stitch(bytes));
    println!("wasm3 {}", wasm3(bytes));
    println!("wasmi {}", wasmi(bytes));
    println!("wasmtime {}", wasmtime(bytes));
}
