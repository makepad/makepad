use {
    criterion::{black_box, criterion_group, criterion_main, Criterion},
    wast::{parser, parser::ParseBuffer, Wat},
};

fn new_stitch_store_and_instance(
    bytes: &[u8],
) -> (makepad_stitch::Store, makepad_stitch::Instance) {
    use makepad_stitch::{Engine, Linker, Module, Store};

    let engine = Engine::new();
    let mut store = Store::new(engine);
    let module = Module::new(store.engine(), &bytes).unwrap();
    let instance = Linker::new().instantiate(&mut store, &module).unwrap();
    (store, instance)
}

fn new_wasm3_module(bytes: &[u8], f: impl FnOnce(wasm3::Module)) {
    use wasm3::Environment;

    let environment = Environment::new().unwrap();
    let runtime = environment.create_runtime(1024 * 1024).unwrap();
    let module = runtime.parse_and_load_module(bytes).unwrap();
    f(module)
}

fn new_wasmi_store_and_instance(bytes: &[u8]) -> (wasmi::Store<()>, wasmi::Instance) {
    use wasmi::{Config, Engine, Linker, Module, Store};

    let config = Config::default();
    let engine = Engine::new(&config);
    let mut store = Store::new(&engine, ());
    let module = Module::new(store.engine(), bytes).unwrap();
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &module)
        .unwrap();
    let instance = instance.start(&mut store).unwrap();
    (store, instance)
}

fn fac_iter(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fac_iter.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let n = 32;
    let mut group = c.benchmark_group("fac_iter");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fac_iter = instance.exported_func("fac_iter").unwrap();

        b.iter(|| {
            use makepad_stitch::Val;

            fac_iter
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(n)]),
                    black_box(&mut [Val::I64(0)]),
                )
                .unwrap();
        })
    });
    group.bench_function("wasm3", |b| {
        new_wasm3_module(&bytes, |module| {
            let fac_iter = module.find_function::<i64, i64>("fac_iter").unwrap();
            b.iter(|| {
                fac_iter.call(black_box(n as i64)).unwrap();
            })
        });
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fac_iter = instance.get_func(&store, "fac_iter").unwrap();

        b.iter(|| {
            use wasmi::Value;

            fac_iter
                .call(
                    black_box(&mut store),
                    black_box(&[Value::I64(n)]),
                    black_box(&mut [Value::I64(0)]),
                )
                .unwrap();
        })
    });
}

fn fac_rec(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fac_rec.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let n = 32;
    let mut group = c.benchmark_group("fac_rec");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fac_rec = instance.exported_func("fac_rec").unwrap();

        b.iter(|| {
            use makepad_stitch::Val;

            fac_rec
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(n)]),
                    black_box(&mut [Val::I64(0)]),
                )
                .unwrap();
        })
    });
    group.bench_function("wasm3", |b| {
        new_wasm3_module(&bytes, |module| {
            let fac_rec = module.find_function::<i64, i64>("fac_rec").unwrap();
            b.iter(|| {
                fac_rec.call(black_box(n as i64)).unwrap();
            })
        });
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fac_rec = instance.get_func(&store, "fac_rec").unwrap();

        b.iter(|| {
            use wasmi::Value;

            fac_rec
                .call(
                    black_box(&mut store),
                    black_box(&[Value::I64(n)]),
                    black_box(&mut [Value::I64(0)]),
                )
                .unwrap();
        })
    });
}

fn fib(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fib.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let n = 32;
    let mut group = c.benchmark_group("fib");
    group.bench_function("stitch", |b| {
        use makepad_stitch::Val;

        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fib = instance.exported_func("fib").unwrap();

        b.iter(|| {
            let mut results = [Val::I64(0)];
            fib.call(
                black_box(&mut store),
                black_box(&[Val::I64(n as i64)]),
                black_box(&mut results),
            )
            .unwrap();
            assert_eq!(results[0].to_i64().unwrap(), 2178309);
        })
    });
    group.bench_function("wasm3", |b| {
        new_wasm3_module(&bytes, |module| {
            let fib = module.find_function::<i64, i64>("fib").unwrap();
            b.iter(|| {
                fib.call(black_box(n as i64)).unwrap();
            })
        });
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fib = instance.get_func(&store, "fib").unwrap();

        b.iter(|| {
            use wasmi::Value;

            fib.call(
                black_box(&mut store),
                black_box(&[Value::I64(n as i64)]),
                black_box(&mut [Value::I64(0)]),
            )
            .unwrap();
        })
    });
}

fn fill(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fill.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let idx = 0;
    let val = 42;
    let count = 1_048_576;
    let mut group = c.benchmark_group("fill");
    group.bench_function("stitch", |b| {
        use makepad_stitch::Val;

        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fill = instance.exported_func("fill").unwrap();

        b.iter(|| {
            fill.call(
                black_box(&mut store),
                black_box(&[
                    Val::I32(idx as i32),
                    Val::I32(val as i32),
                    Val::I32(count as i32),
                ]),
                black_box(&mut []),
            )
            .unwrap();
        });
    });
    group.bench_function("wasm3", |b| {
        new_wasm3_module(&bytes, |module| {
            let fill = module.find_function::<(i32, i32, i32), ()>("fill").unwrap();
            b.iter(|| {
                fill.call(
                    black_box(idx as i32),
                    black_box(val as i32),
                    black_box(count as i32),
                )
            })
        });
    });
    group.bench_function("wasmi", |b| {
        use wasmi::Value;

        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fill = instance.get_func(&store, "fill").unwrap();

        b.iter(|| {
            fill.call(
                black_box(&mut store),
                black_box(&[
                    Value::I32(idx as i32),
                    Value::I32(val as i32),
                    Value::I32(count as i32),
                ]),
                black_box(&mut []),
            )
            .unwrap();
        })
    });
}

fn sum(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/sum.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let idx = 0;
    let count = 1_048_576;
    let mut group = c.benchmark_group("sum");
    group.bench_function("stitch", |b| {
        use makepad_stitch::Val;

        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let memory = instance.exported_mem("memory").unwrap();
        let sum = instance.exported_func("sum").unwrap();

        for (idx, byte) in &mut memory.bytes_mut(&mut store)[..count].iter_mut().enumerate() {
            let val = (idx % 256) as u8;
            *byte = val;
        }
        b.iter(|| {
            sum.call(
                black_box(&mut store),
                black_box(&[Val::I32(idx as i32), Val::I32(count as i32)]),
                black_box(&mut [Val::I64(0)]),
            )
            .unwrap();
        });
    });
    group.bench_function("wasm3", |b| {
        new_wasm3_module(&bytes, |module| {
            let sum = module.find_function::<(i32, i32), i64>("sum").unwrap();
            b.iter(|| sum.call(black_box(idx as i32), black_box(count as i32)))
        });
    });
    group.bench_function("wasmi", |b| {
        use wasmi::Value;

        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let memory = instance.get_memory(&store, "memory").unwrap();
        let sum = instance.get_func(&store, "sum").unwrap();

        for (idx, byte) in &mut memory.data_mut(&mut store)[..count].iter_mut().enumerate() {
            let val = (idx % 256) as u8;
            *byte = val;
        }
        b.iter(|| {
            sum.call(
                black_box(&mut store),
                black_box(&[Value::I32(idx as i32), Value::I32(count as i32)]),
                black_box(&mut [Value::I64(0)]),
            )
            .unwrap();
        })
    });
}

criterion_group!(benches, fac_iter); // fac_ref, fib, fill, sum);
criterion_main!(benches);
