use {
    criterion::{black_box, criterion_group, criterion_main, Criterion},
    makepad_collections::BTreeString,
};

pub fn bench(c: &mut Criterion) {
    let string = ('\0'..=char::MAX).cycle().take(1024 * 1024).collect::<String>();
    let btree_string = BTreeString::from(&string);

    let mut group = c.benchmark_group("bytes");
    group.bench_function("BTreeString", |b| b.iter(|| {
        for byte in btree_string.bytes_rev() {
            black_box(byte);
        }    
    }));
    group.bench_function("String", |b| b.iter(|| {
        for byte in string.bytes().rev() {
            black_box(byte);
        }    
    }));
    group.finish();

    let mut group = c.benchmark_group("bytes_rev");
    group.bench_function("BTreeString", |b| b.iter(|| {
        for byte in btree_string.bytes_rev() {
            black_box(byte);
        }    
    }));
    group.bench_function("String", |b| b.iter(|| {
        for byte in string.bytes().rev() {
            black_box(byte);
        }    
    }));
    group.finish();

    let mut group = c.benchmark_group("chars");
    group.bench_function("BTreeString", |b| b.iter(|| {
        for byte in btree_string.chars() {
            black_box(byte);
        }    
    }));
    group.bench_function("String", |b| b.iter(|| {
        for byte in string.chars() {
            black_box(byte);
        }    
    }));
    group.finish();

    let mut group = c.benchmark_group("chars_rev");
    group.bench_function("BTreeString", |b| b.iter(|| {
        for byte in btree_string.chars_rev() {
            black_box(byte);
        }    
    }));
    group.bench_function("String", |b| b.iter(|| {
        for byte in string.chars().rev() {
            black_box(byte);
        }    
    }));
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);