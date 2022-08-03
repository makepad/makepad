use {
    criterion::{black_box, criterion_group, criterion_main, Criterion},
    makepad_collections::BTreeString,
};

pub fn bytes(c: &mut Criterion) {
    let string = ('\0'..=char::MAX).cycle().take(1024 * 1024).collect::<String>();
    let btree_string = BTreeString::from(&string);
    let mut group = c.benchmark_group("bytes");
    group.bench_function("BTreeString", |b| b.iter(|| {
        for byte in btree_string.bytes() {
            black_box(byte);
        }    
    }));
    group.bench_function("String", |b| b.iter(|| {
        for byte in string.bytes() {
            black_box(byte);
        }    
    }));
}

criterion_group!(benches, bytes);
criterion_main!(benches);