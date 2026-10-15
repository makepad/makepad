//! Real-std differential coverage for both runtime Freeze dispatch paths.
use relax_std_check::slice::sort as port;
use std::cell::Cell;
use std::cmp::Ordering;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

// FORCE_FREEZE is process-global: keep every sort test under this lock.
static FREEZE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn both_freeze_paths<F: FnMut()>(mut test: F) {
    let _lock = FREEZE_TEST_LOCK.lock().unwrap();
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            port::FORCE_FREEZE.store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
    let _reset = Reset;
    for freeze in [false, true] {
        port::FORCE_FREEZE.store(freeze, std::sync::atomic::Ordering::Relaxed);
        test();
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn compare<T: Clone + std::fmt::Debug, F: Fn(&T, &T) -> Ordering>(input: &[T], cmp: F) {
    let mut real = input.to_vec();
    real.sort_by(&cmp);
    let mut stable = input.to_vec();
    port::merge_sort(&mut stable, &mut |a, b| cmp(a, b).is_lt());
    // Debug representation also preserves float NaN bit identities in the separate test below.
    assert_eq!(format!("{stable:?}"), format!("{real:?}"));
    let mut unstable = input.to_vec();
    port::quicksort(&mut unstable, &mut |a, b| cmp(a, b).is_lt());
    assert!(unstable.windows(2).all(|w| !cmp(&w[1], &w[0]).is_lt()));
    let mut before: Vec<_> = input.iter().map(|x| format!("{x:?}")).collect();
    let mut after: Vec<_> = unstable.iter().map(|x| format!("{x:?}")).collect();
    before.sort();
    after.sort();
    assert_eq!(before, after);
    for index in [
        0,
        input.len() / 3,
        input.len() / 2,
        input.len().saturating_sub(1),
    ] {
        if input.is_empty() {
            break;
        }
        let mut v = input.to_vec();
        port::select_nth(&mut v, index, &mut |a, b| cmp(a, b).is_lt());
        assert_eq!(cmp(&v[index], &real[index]), Ordering::Equal);
        assert!(v[..index].iter().all(|x| !cmp(&v[index], x).is_lt()));
        assert!(v[index + 1..].iter().all(|x| !cmp(x, &v[index]).is_lt()));
        let mut selected: Vec<_> = v.iter().map(|x| format!("{x:?}")).collect();
        selected.sort();
        assert_eq!(selected, before);
    }
}

#[test]
fn patterns_and_random_types() {
    both_freeze_paths(patterns_and_random_types_case);
}

fn patterns_and_random_types_case() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut lengths: Vec<usize> = (0..=64).collect();
    lengths.extend([
        63, 64, 65, 127, 128, 511, 512, 513, 4095, 4096, 4097, 100_000,
    ]);
    for _ in 0..12 {
        lengths.push((rng.next() % 100_001) as usize);
    }
    for n in lengths {
        for pattern in 0..7 {
            let input: Vec<u64> = (0..n)
                .map(|i| match pattern {
                    0 => i as u64,
                    1 => (n - i) as u64,
                    2 => (i % 31) as u64,
                    3 => 7,
                    4 => rng.next() % 5,
                    5 => rng.next(),
                    _ => i.min(n - i) as u64,
                })
                .collect();
            compare(&input, u64::cmp);
            compare(
                &input.iter().map(|&x| x as u32).collect::<Vec<_>>(),
                u32::cmp,
            );
            compare(&input.iter().map(|&x| x as i8).collect::<Vec<_>>(), i8::cmp);
            if n <= 513 || pattern == 5 {
                compare(
                    &input.iter().map(|&x| f64::from_bits(x)).collect::<Vec<_>>(),
                    f64::total_cmp,
                );
                compare(
                    &input
                        .iter()
                        .map(|x| format!("key-{x:016x}"))
                        .collect::<Vec<_>>(),
                    String::cmp,
                );
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    key: u32,
    serial: usize,
    text: String,
}

#[test]
fn stability_keys_and_binary_search() {
    both_freeze_paths(stability_keys_and_binary_search_case);
}

fn stability_keys_and_binary_search_case() {
    let mut rng = Rng(0xdeadbeef12345678);
    for n in (0..=64).chain([65, 513, 4097, 100_000]) {
        let input: Vec<_> = (0..n)
            .map(|i| Item {
                key: (rng.next() % 17) as u32,
                serial: i,
                text: i.to_string(),
            })
            .collect();
        let mut real = input.clone();
        real.sort_by_key(|x| x.key);
        let mut actual = input.clone();
        port::merge_sort(&mut actual, &mut |a, b| a.key < b.key);
        assert_eq!(actual, real);
        let mut cached = input.clone();
        let mut calls = 0;
        port::sort_by_cached_key(&mut cached, |x| {
            calls += 1;
            x.key
        });
        assert_eq!(cached, real);
        assert_eq!(calls, if n < 2 { 0 } else { n });
        for key in 0..=18 {
            assert_eq!(
                relax_std_check::slice::binary_search_by(&actual, |x| x.key.cmp(&key)),
                real.binary_search_by_key(&key, |x| x.key)
            );
        }
    }
    for n in 0..=64 {
        for key in -2..=9 {
            let v: Vec<i32> = (0..n).map(|i| i / 6).collect();
            let mut real_calls = Vec::new();
            let mut port_calls = Vec::new();
            let real = v.binary_search_by(|x| {
                real_calls.push(*x);
                x.cmp(&key)
            });
            let actual = relax_std_check::slice::binary_search_by(&v, |x| {
                port_calls.push(*x);
                x.cmp(&key)
            });
            assert_eq!(actual, real);
            assert_eq!(port_calls, real_calls);
        }
    }
}

#[test]
fn exhaustive_ternary_and_selection_indices() {
    both_freeze_paths(exhaustive_ternary_and_selection_indices_case);
}

fn exhaustive_ternary_and_selection_indices_case() {
    for n in 0..=8 {
        for bits in 0..3usize.pow(n as u32) {
            let mut code = bits;
            let v: Vec<i8> = (0..n)
                .map(|_| {
                    let x = (code % 3) as i8;
                    code /= 3;
                    x
                })
                .collect();
            let mut sorted = v.clone();
            sorted.sort();
            let mut actual = v.clone();
            port::merge_sort(&mut actual, &mut |a, b| a < b);
            assert_eq!(actual, sorted);
            let mut actual = v.clone();
            port::quicksort(&mut actual, &mut |a, b| a < b);
            assert_eq!(actual, sorted);
            for i in 0..n {
                let mut actual = v.clone();
                port::select_nth(&mut actual, i, &mut |a, b| a < b);
                assert_eq!(actual[i], sorted[i]);
                assert!(actual[..i].iter().all(|x| *x <= actual[i]));
                assert!(actual[i + 1..].iter().all(|x| *x >= actual[i]));
            }
        }
    }
}

struct Owned<const PAD: usize> {
    key: u64,
    id: usize,
    touches: Cell<usize>,
    drops: Rc<Vec<Cell<usize>>>,
    padding: [u8; PAD],
}
impl<const PAD: usize> Drop for Owned<PAD> {
    fn drop(&mut self) {
        self.drops[self.id].set(self.drops[self.id].get() + 1);
    }
}

fn unwind_cases<const PAD: usize>() {
    for algorithm in 0..4 {
        for pattern in 0..4 {
            let n = match pattern {
                0 => 19,
                3 => 32,
                _ => 193,
            };
            // Every comparison boundary is tested, covering scratch partition,
            // forward/backward merge, insertion gaps, both cyclic partitions.
            let mut fail_at = 0;
            loop {
                let drops = Rc::new((0..n).map(|_| Cell::new(0)).collect::<Vec<_>>());
                let mut rng = Rng(0x87654321);
                let mut v: Vec<_> = (0..n)
                    .map(|id| Owned {
                        key: if pattern == 2 {
                            if id < 129 {
                                id as u64 + 1000
                            } else {
                                id as u64
                            }
                        } else {
                            rng.next() % 31
                        },
                        id,
                        touches: Cell::new(0),
                        drops: drops.clone(),
                        padding: [0; PAD],
                    })
                    .collect();
                let freeze = port::FORCE_FREEZE.load(std::sync::atomic::Ordering::Relaxed);
                // Forced Freeze must not mutate the compared objects: observe
                // touches externally instead. The false path still checks Cell.
                let touches: Vec<_> = (0..n).map(|_| Cell::new(0usize)).collect();
                let mut calls = 0;
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let mut less = |a: &Owned<PAD>, b: &Owned<PAD>| {
                        if freeze {
                            touches[a.id].set(touches[a.id].get() + 1);
                            touches[b.id].set(touches[b.id].get() + 1);
                        } else {
                            a.touches.set(a.touches.get() + 1);
                            b.touches.set(b.touches.get() + 1);
                        }
                        calls += 1;
                        if calls == fail_at + 1 {
                            panic!("comparison panic");
                        }
                        a.key < b.key
                    };
                    match algorithm {
                        0 => port::merge_sort(&mut v, &mut less),
                        1 => port::quicksort(&mut v, &mut less),
                        2 => port::select_nth(&mut v, n / 2, &mut less),
                        _ => port::heapsort(&mut v, &mut less),
                    }
                }));
                assert!(drops.iter().all(|x| x.get() == 0));
                let mut ids: Vec<_> = v.iter().map(|x| x.id).collect();
                ids.sort();
                assert_eq!(ids, (0..n).collect::<Vec<_>>());
                if freeze {
                    assert_eq!(touches.iter().map(Cell::get).sum::<usize>(), calls * 2);
                } else {
                    assert_eq!(v.iter().map(|x| x.touches.get()).sum::<usize>(), calls * 2);
                }
                assert!(v.iter().all(|x| x.padding.iter().all(|&b| b == 0)));
                drop(v);
                assert!(drops.iter().all(|x| x.get() == 1));
                if outcome.is_ok() {
                    break;
                }
                fail_at += 1;
            }
        }
    }
}

#[test]
fn panic_permutation_drop_and_interior_mutation() {
    both_freeze_paths(panic_permutation_drop_and_interior_mutation_case);
}

fn panic_permutation_drop_and_interior_mutation_case() {
    unwind_cases::<0>();
    unwind_cases::<128>();
}

#[test]
fn float_bits_zst_and_overaligned() {
    both_freeze_paths(float_bits_zst_and_overaligned_case);
}

fn float_bits_zst_and_overaligned_case() {
    let bits = [
        0,
        1 << 63,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000001,
        0xfff8000000000020,
        0x7ff0000000000001,
    ];
    let mut v: Vec<f64> = bits
        .into_iter()
        .cycle()
        .take(777)
        .map(f64::from_bits)
        .collect();
    let mut real = v.clone();
    real.sort_by(f64::total_cmp);
    port::merge_sort(&mut v, &mut |a, b| a.total_cmp(b).is_lt());
    assert_eq!(
        v.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        real.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
    let mut z = [(); 100];
    port::merge_sort(&mut z, &mut |_, _| panic!("ZST comparator"));
    port::quicksort(&mut z, &mut |_, _| panic!("ZST comparator"));
    port::select_nth(&mut z, 50, &mut |_, _| panic!("ZST comparator"));
    assert!(catch_unwind(AssertUnwindSafe(|| port::select_nth(
        &mut z,
        100,
        &mut |_, _| false
    )))
    .is_err());
    #[repr(align(8192))]
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Aligned(u64);
    let mut v: Vec<_> = (0..65).map(|x| Aligned((x * 31) % 17)).collect();
    let mut real = v.clone();
    real.sort();
    port::merge_sort(&mut v, &mut |a, b| a < b);
    assert_eq!(v, real);
}

#[test]
fn heap_scratch_scaling_and_long_runs() {
    both_freeze_paths(heap_scratch_scaling_and_long_runs_case);
}

fn heap_scratch_scaling_and_long_runs_case() {
    let mut rng = Rng(0xabab01234567);
    // Both sides of the 8 MB full-scratch cutoff, and the half-scratch regime.
    for n in [1_000_001usize, 2_000_001] {
        let mut v: Vec<_> = (0..n).map(|_| rng.next()).collect();
        let mut real = v.clone();
        real.sort();
        port::merge_sort(&mut v, &mut |a, b| a < b);
        assert_eq!(v, real);
    }
    for run in [31usize, 64, 129, 511, 2049] {
        let mut v: Vec<_> = (0..100_000).map(|i| ((i / run) % 7, i % run, i)).collect();
        let mut real = v.clone();
        real.sort_by_key(|x| (x.0, x.1));
        port::merge_sort(&mut v, &mut |a, b| (a.0, a.1) < (b.0, b.1));
        assert_eq!(v, real);
    }
}

#[test]
#[ignore = "optional rustc -O timing, best of five; excludes input cloning"]
fn timing_best_of_five() {
    use std::hint::black_box;
    use std::time::{Duration, Instant};
    let mut rng = Rng(0x71728394);
    let random: Vec<u64> = (0..100_000).map(|_| rng.next()).collect();
    for pattern in 0..4 {
        let mut input = random.clone();
        match pattern {
            1 => input.sort_unstable(),
            2 => {
                input.sort_unstable();
                input.reverse();
            }
            3 => {
                for x in &mut input {
                    *x %= 7;
                }
            }
            _ => (),
        }
        let mut best = [Duration::MAX; 4];
        for _ in 0..5 {
            for algorithm in 0..4 {
                let mut v = input.clone();
                let start = Instant::now();
                match algorithm {
                    0 => v.sort(),
                    1 => port::merge_sort(&mut v, &mut |a, b| a < b),
                    2 => v.sort_unstable(),
                    _ => port::quicksort(&mut v, &mut |a, b| a < b),
                }
                best[algorithm] = best[algorithm].min(start.elapsed());
                black_box(&v);
            }
        }
        println!(
            "pattern {pattern}: stable real {:?} port {:?}; unstable real {:?} port {:?}",
            best[0], best[1], best[2], best[3]
        );
    }
    let input: Vec<_> = random
        .iter()
        .enumerate()
        .map(|(i, &x)| Item {
            key: (x % 1024) as u32,
            serial: i,
            text: i.to_string(),
        })
        .collect();
    let mut best = [Duration::MAX; 2];
    for _ in 0..5 {
        for algorithm in 0..2 {
            let mut v = input.clone();
            let start = Instant::now();
            if algorithm == 0 {
                v.sort_by_key(|x| x.key);
            } else {
                port::merge_sort(&mut v, &mut |a, b| a.key < b.key);
            }
            best[algorithm] = best[algorithm].min(start.elapsed());
            black_box(&v);
        }
    }
    println!("struct key: stable real {:?} port {:?}", best[0], best[1]);
}

#[test]
fn copy_struct_keys() {
    both_freeze_paths(|| {
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct CopyItem {
            key: u32,
            value: f32,
        }
        #[derive(Clone, Copy, Debug, PartialEq)]
        struct WideItem {
            key: u32,
            serial: usize,
            payload: [u64; 4],
        }
        let mut rng = Rng(0x128797);
        for n in (0..=64).chain([65, 513, 4097, 100_000]) {
            let input: Vec<_> = (0..n)
                .map(|i| CopyItem {
                    key: (rng.next() % 11) as u32,
                    value: i as f32,
                })
                .collect();
            let mut real = input.clone();
            real.sort_by_key(|x| x.key);
            let mut actual = input.clone();
            port::merge_sort(&mut actual, &mut |a, b| a.key < b.key);
            assert_eq!(actual, real);
            compare(&input, |a, b| a.key.cmp(&b.key));
            let wide: Vec<_> = input
                .iter()
                .enumerate()
                .map(|(i, x)| WideItem {
                    key: x.key,
                    serial: i,
                    payload: [i as u64; 4],
                })
                .collect();
            compare(&wide, |a, b| a.key.cmp(&b.key));
        }
    });
}

#[test]
fn copy_comparator_panics() {
    both_freeze_paths(|| {
        // Eight-byte Copy elements exercise both sort8 and the sorting networks.
        for n in [21, 32, 33, 48, 64, 193] {
            let input: Vec<_> = (0..n).map(|i| (((i * 31) % 11) as u32, i as u32)).collect();
            for stable in [false, true] {
                let mut fail_at = 0;
                loop {
                    let mut v = input.clone();
                    let mut calls = 0;
                    let outcome = catch_unwind(AssertUnwindSafe(|| {
                        let mut less = |a: &(u32, u32), b: &(u32, u32)| {
                            calls += 1;
                            if calls == fail_at + 1 {
                                panic!("comparison panic");
                            }
                            a.0 < b.0
                        };
                        if stable {
                            port::merge_sort(&mut v, &mut less);
                        } else {
                            port::quicksort(&mut v, &mut less);
                        }
                    }));
                    let mut ids: Vec<_> = v.iter().map(|x| x.1).collect();
                    ids.sort();
                    assert_eq!(ids, (0..n as u32).collect::<Vec<_>>());
                    assert!(v.iter().all(|x| x.0 == (x.1 * 31) % 11));
                    if outcome.is_ok() {
                        break;
                    }
                    fail_at += 1;
                }
            }
        }
    });
}
