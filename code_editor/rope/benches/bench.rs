use {
    criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion},
    makepad_rope::Rope,
};

fn bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes");
    for char_len in [1024, 32 * 1024, 1024 * 1024, 32 * 1024 * 1024] {
        group.bench_with_input(
            BenchmarkId::new("Rope", char_len),
            &char_len,
            |b, &char_len| {
                let rope = ('\0'..=char::MAX).cycle().take(char_len).collect::<Rope>();
                b.iter(|| {
                    for byte in rope.bytes() {
                        black_box(byte);
                    }
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("String", char_len),
            &char_len,
            |b, &char_len| {
                let string = ('\0'..=char::MAX)
                    .cycle()
                    .take(char_len)
                    .collect::<String>();
                b.iter(|| {
                    for byte in string.bytes() {
                        black_box(byte);
                    }
                })
            },
        );
    }
    group.finish();
}

fn bytes_rev(c: &mut Criterion) {
    let mut group = c.benchmark_group("bytes_rev");
    for char_len in [1024, 32 * 1024, 1024 * 1024, 32 * 1024 * 1024] {
        group.bench_with_input(
            BenchmarkId::new("Rope", char_len),
            &char_len,
            |b, &char_len| {
                let rope = ('\0'..=char::MAX).cycle().take(char_len).collect::<Rope>();
                b.iter(|| {
                    for byte in rope.bytes_rev() {
                        black_box(byte);
                    }
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("String", char_len),
            &char_len,
            |b, &char_len| {
                let string = ('\0'..=char::MAX)
                    .cycle()
                    .take(char_len)
                    .collect::<String>();
                b.iter(|| {
                    for byte in string.bytes().rev() {
                        black_box(byte);
                    }
                })
            },
        );
    }
    group.finish();
}

fn chars(c: &mut Criterion) {
    let mut group = c.benchmark_group("chars");
    for char_len in [1024, 32 * 1024, 1024 * 1024, 32 * 1024 * 1024] {
        group.bench_with_input(
            BenchmarkId::new("Rope", char_len),
            &char_len,
            |b, &char_len| {
                let rope = ('\0'..=char::MAX).cycle().take(char_len).collect::<Rope>();
                b.iter(|| {
                    for ch in rope.chars() {
                        black_box(ch);
                    }
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("String", char_len),
            &char_len,
            |b, &char_len| {
                let string = ('\0'..=char::MAX)
                    .cycle()
                    .take(char_len)
                    .collect::<String>();
                b.iter(|| {
                    for ch in string.chars() {
                        black_box(ch);
                    }
                })
            },
        );
    }
    group.finish();
}

fn chars_rev(c: &mut Criterion) {
    let mut group = c.benchmark_group("chars_rev");
    for char_len in [1024, 32 * 1024, 1024 * 1024, 32 * 1024 * 1024] {
        group.bench_with_input(
            BenchmarkId::new("Rope", char_len),
            &char_len,
            |b, &char_len| {
                let rope = ('\0'..=char::MAX).cycle().take(char_len).collect::<Rope>();
                b.iter(|| {
                    for ch in rope.chars_rev() {
                        black_box(ch);
                    }
                })
            },
        );
        group.bench_with_input(
            BenchmarkId::new("String", char_len),
            &char_len,
            |b, &char_len| {
                let string = ('\0'..=char::MAX)
                    .cycle()
                    .take(char_len)
                    .collect::<String>();
                b.iter(|| {
                    for ch in string.chars().rev() {
                        black_box(ch);
                    }
                })
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bytes, bytes_rev, chars, chars_rev);
criterion_main!(benches);
