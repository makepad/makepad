use {
    criterion::{criterion_group, criterion_main, Criterion},
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

fn count_until(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/count_until.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let mut group = c.benchmark_group("count_until");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let count_until = instance.exported_func("count_until").unwrap();

        b.iter(|| {
            count_until
                .call(
                    &mut store,
                    &[makepad_stitch::Val::I64(1_0000_000)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let count_until = instance
            .get_func(&store, "count_until")
            .unwrap();

        b.iter(|| {
            count_until
                .call(
                    &mut store,
                    &[wasmi::Value::I64(1_0000_000)],
                    &mut [wasmi::Value::I64(0)],
                )
                .unwrap();
        })
    });
}

fn fac(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fac.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let mut group = c.benchmark_group("fac");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fac = instance.exported_func("fac").unwrap();

        b.iter(|| {
            fac
                .call(
                    &mut store,
                    &[makepad_stitch::Val::I64(25)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fac = instance.get_func(&store, "fac").unwrap();

        b.iter(|| {
            fac
                .call(
                    &mut store,
                    &[wasmi::Value::I64(25)],
                    &mut [wasmi::Value::I64(0)],
                )
                .unwrap();
        })
    });
}

fn fib(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/fib.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let mut group = c.benchmark_group("fib");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let fib = instance.exported_func("fib").unwrap();

        b.iter(|| {
            fib
                .call(
                    &mut store,
                    &[makepad_stitch::Val::I64(25)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let fib = instance.get_func(&store, "fib").unwrap();

        b.iter(|| {
            fib
                .call(
                    &mut store,
                    &[wasmi::Value::I64(25)],
                    &mut [wasmi::Value::I64(0)],
                )
                .unwrap();
        })
    });
}

fn memory_fill(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/memory_fill.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let mut group = c.benchmark_group("fib");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let memory = instance.exported_mem("memory").unwrap();
        let memory_fill = instance.exported_func("memory_fill").unwrap();

        b.iter(|| {
            memory_fill
                .call(
                    &mut store,
                    &[makepad_stitch::Val::I32(42), makepad_stitch::Val::I32(1_048_576)],
                    &mut [],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let memory = instance.get_memory(&store, "memory").unwrap();
        let memory_fill = instance.get_func(&store, "memory_fill").unwrap();

        b.iter(|| {
            memory_fill
                .call(
                    &mut store,
                    &[wasmi::Value::I32(42), wasmi::Value::I32(1_048_576)],
                    &mut [],
                )
                .unwrap();
        })
    });
}

fn memory_sum(c: &mut Criterion) {
    let buffer = ParseBuffer::new(include_str!("wat/memory_sum.wat")).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();

    let mut group = c.benchmark_group("fib");
    group.bench_function("stitch", |b| {
        let (mut store, instance) = new_stitch_store_and_instance(&bytes);
        let memory = instance.exported_mem("memory").unwrap();
        let memory_sum = instance.exported_func("memory_sum").unwrap();

        let count = 1_048_576;
        for (index, byte_slot) in &mut memory.bytes_mut(&mut store)[..count].iter_mut().enumerate() {
            let byte = (index % 256) as u8;
            *byte_slot = byte;
        }
        b.iter(|| {
            memory_sum
                .call(
                    &mut store,
                    &[makepad_stitch::Val::I32(count as i32)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        let (mut store, instance) = new_wasmi_store_and_instance(&bytes);
        let memory = instance.get_memory(&store, "memory").unwrap();
        let memory_sum = instance.get_func(&store, "memory_sum").unwrap();

        let count = 1_048_576;
        for (index, byte_slot) in &mut memory.data_mut(&mut store)[..count].iter_mut().enumerate() {
            let byte = (index % 256) as u8;
            *byte_slot = byte;
        }
        b.iter(|| {
            memory_sum
                .call(
                    &mut store,
                    &[wasmi::Value::I32(count as i32)],
                    &mut [wasmi::Value::I64(0)],
                )
                .unwrap();
        })
    });
}

criterion_group!(benches, memory_fill);
criterion_main!(benches);
