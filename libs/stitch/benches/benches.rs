use {
    criterion::{black_box, criterion_group, criterion_main, Criterion},
    makepad_stitch::{Engine, Linker, Module, Store, Val},
    wast::{parser, parser::ParseBuffer, Wat},
};

fn new_store_and_instance(input: &str) -> (makepad_stitch::Store, makepad_stitch::Instance) {
    let engine = Engine::new();
    let mut store = Store::new(engine);
    let buffer = ParseBuffer::new(input).unwrap();
    let mut wat = parser::parse::<Wat>(&buffer).unwrap();
    let bytes = wat.encode().unwrap();
    let module = Module::new(store.engine(), &bytes).unwrap();
    let instance = Linker::new().instantiate(&mut store, &module).unwrap();
    (store, instance)
}

fn fac_iter(c: &mut Criterion) {
    c.bench_function("fac_iter", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/fac_iter.wat"));
        let fac_iter = instance.exported_func("fac_iter").unwrap();

        b.iter(|| {
            fac_iter
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(1_048_576)]),
                    black_box(&mut [Val::I64(0)]),
                )
                .unwrap();
        })
    });
}

fn fac_rec(c: &mut Criterion) {
    c.bench_function("fac_rec", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/fac_rec.wat"));
        let fac_rec = instance.exported_func("fac_rec").unwrap();

        b.iter(|| {
            fac_rec
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(32)]),
                    black_box(&mut [Val::I64(0)]),
                )
                .unwrap();
        })
    });
}

fn fib_iter(c: &mut Criterion) {
    c.bench_function("fib_iter", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/fib_iter.wat"));
        let fib_iter = instance.exported_func("fib_iter").unwrap();

        b.iter(|| {
            let mut results = [Val::I64(0)];
            fib_iter
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(1_048_576)]),
                    black_box(&mut results),
                )
                .unwrap();
        })
    });
}

fn fib_rec(c: &mut Criterion) {
    c.bench_function("fib_rec", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/fib_rec.wat"));
        let fib_rec = instance.exported_func("fib_rec").unwrap();

        b.iter(|| {
            let mut results = [Val::I64(0)];
            fib_rec
                .call(
                    black_box(&mut store),
                    black_box(&[Val::I64(32)]),
                    black_box(&mut results),
                )
                .unwrap();
        })
    });
}

fn fill(c: &mut Criterion) {
    c.bench_function("fill", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/fill.wat"));
        let fill = instance.exported_func("fill").unwrap();

        b.iter(|| {
            fill.call(
                black_box(&mut store),
                black_box(&[Val::I32(0), Val::I32(42), Val::I32(1_048_576)]),
                black_box(&mut []),
            )
            .unwrap();
        });
    });
}

fn sum(c: &mut Criterion) {
    c.bench_function("sum", |b| {
        let (mut store, instance) = new_store_and_instance(include_str!("wat/sum.wat"));
        let memory = instance.exported_mem("memory").unwrap();
        let sum = instance.exported_func("sum").unwrap();

        let idx = 0;
        let count = 1_048_576;
        for (idx, byte) in &mut memory.bytes_mut(&mut store)[idx..][..count]
            .iter_mut()
            .enumerate()
        {
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
}

criterion_group!(benches, fac_iter, fac_rec, fib_iter, fib_rec, fill, sum);
criterion_main!(benches);
