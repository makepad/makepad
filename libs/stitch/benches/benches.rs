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

    let (mut stitch_store, stitch_instance) = new_stitch_store_and_instance(&bytes);
    let stitch_count_until = stitch_instance.exported_func("count_until").unwrap();

    let (mut wasmi_store, wasmi_instance) = new_wasmi_store_and_instance(&bytes);
    let wasmi_count_until = wasmi_instance.get_func(&wasmi_store, "count_until").unwrap();

    let mut group = c.benchmark_group("count_until");
    group.bench_function("stitch", |b| {
        b.iter(|| {
            stitch_count_until
                .call(
                    &mut stitch_store,
                    &[makepad_stitch::Val::I64(1_0000_000)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        b.iter(|| {
            wasmi_count_until
                .call(
                    &mut wasmi_store,
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

    let (mut stitch_store, stitch_instance) = new_stitch_store_and_instance(&bytes);
    let stitch_fac = stitch_instance.exported_func("fac").unwrap();

    let (mut wasmi_store, wasmi_instance) = new_wasmi_store_and_instance(&bytes);
    let wasmi_fac = wasmi_instance.get_func(&wasmi_store, "fac").unwrap();

    let mut group = c.benchmark_group("fac");
    group.bench_function("stitch", |b| {
        b.iter(|| {
            stitch_fac
                .call(
                    &mut stitch_store,
                    &[makepad_stitch::Val::I64(25)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        b.iter(|| {
            wasmi_fac
                .call(
                    &mut wasmi_store,
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

    let (mut stitch_store, stitch_instance) = new_stitch_store_and_instance(&bytes);
    let stitch_fib = stitch_instance.exported_func("fib").unwrap();

    let (mut wasmi_store, wasmi_instance) = new_wasmi_store_and_instance(&bytes);
    let wasmi_fib = wasmi_instance.get_func(&wasmi_store, "fib").unwrap();

    let mut group = c.benchmark_group("fib");
    group.bench_function("stitch", |b| {
        b.iter(|| {
            stitch_fib
                .call(
                    &mut stitch_store,
                    &[makepad_stitch::Val::I64(25)],
                    &mut [makepad_stitch::Val::I64(0)],
                )
                .unwrap();
        })
    });
    group.bench_function("wasmi", |b| {
        b.iter(|| {
            wasmi_fib
                .call(
                    &mut wasmi_store,
                    &[wasmi::Value::I64(25)],
                    &mut [wasmi::Value::I64(0)],
                )
                .unwrap();
        })
    });
}

criterion_group!(benches, count_until, fac, fib);
criterion_main!(benches);
