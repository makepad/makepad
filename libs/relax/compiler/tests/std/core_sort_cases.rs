// Sorting, selection, stability, search traces and panic-safe element ownership.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::cell::Cell;
use std::cmp::Ordering;
use std::fmt::{Debug, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

struct D(u64, u32);
impl D {
    fn new() -> D {
        D(0xcbf29ce484222325, 0)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    fn obs<T: Debug + ?Sized>(&mut self, v: &T) {
        write!(self, "{:?}", v).unwrap();
        self.bytes(&[0xff]);
        self.1 += 1;
    }
    fn line(&self, tag: &str) {
        println!("| {} {} {:016x}", tag, self.1, self.0);
    }
}
impl Write for D {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.bytes(s.as_bytes());
        Ok(())
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

fn compare<T: Clone + Debug, F: Fn(&T, &T) -> Ordering>(input: &[T], cmp: F, d: &mut D) {
    let mut stable = input.to_vec();
    stable.sort_by(&cmp);
    d.obs(&stable);
    let mut unstable = input.to_vec();
    unstable.sort_unstable_by(&cmp);
    assert!(unstable.windows(2).all(|w| !cmp(&w[1], &w[0]).is_lt()));
    let mut before: Vec<_> = input.iter().map(|x| format!("{x:?}")).collect();
    let mut after: Vec<_> = unstable.iter().map(|x| format!("{x:?}")).collect();
    before.sort();
    after.sort();
    assert_eq!(before, after);
    d.obs(&after);
    for index in [0, input.len() / 3, input.len() / 2, input.len().saturating_sub(1)] {
        if input.is_empty() {
            break;
        }
        let mut v = input.to_vec();
        let _ = v.select_nth_unstable_by(index, &cmp);
        let rank = cmp(&v[index], &stable[index]);
        assert_eq!(rank, Ordering::Equal);
        d.obs(&rank);
        assert!(v[..index].iter().all(|x| !cmp(&v[index], x).is_lt()));
        assert!(v[index + 1..].iter().all(|x| !cmp(x, &v[index]).is_lt()));
        let mut selected: Vec<_> = v.iter().map(|x| format!("{x:?}")).collect();
        selected.sort();
        assert_eq!(selected, before);
        d.obs(&selected);
    }
}

fn patterns<T: Clone + Debug>(
    tag: &str,
    pattern: usize,
    convert: impl Fn(u64) -> T,
    cmp: impl Fn(&T, &T) -> Ordering,
) {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut lengths: Vec<usize> = (0..=64).collect();
    lengths.extend([63, 64, 65, 127, 128, 511, 512, 513, 4095, 4096, 4097]);
    // One random length supplements the complete list of boundary lengths.
    lengths.push((rng.next() % 10_001) as usize);
    let mut d = D::new();
    for n in lengths {
        if (tag == "f64" || tag == "string") && n > 513 && pattern != 5 {
            continue;
        }
        let input: Vec<_> = (0..n)
            .map(|i| {
                convert(match pattern {
                    0 => i as u64,
                    1 => (n - i) as u64,
                    2 => (i % 31) as u64,
                    3 => 7,
                    4 => rng.next() % 5,
                    5 => rng.next(),
                    _ => i.min(n - i) as u64,
                })
            })
            .collect();
        compare(&input, &cmp, &mut d);
    }
    d.line(&format!("sort {} pattern {}", tag, pattern));
}

macro_rules! pattern_test {
    ($name:ident, $tag:expr, $pattern:expr, $convert:expr, $cmp:expr) => {
        #[test]
        fn $name() {
            patterns($tag, $pattern, $convert, $cmp);
        }
    };
}
pattern_test!(a01_patterns_u64_0, "u64", 0, |x| x, u64::cmp);
pattern_test!(a02_patterns_u64_1, "u64", 1, |x| x, u64::cmp);
pattern_test!(a03_patterns_u64_2, "u64", 2, |x| x, u64::cmp);
pattern_test!(a04_patterns_u64_3, "u64", 3, |x| x, u64::cmp);
pattern_test!(a05_patterns_u64_4, "u64", 4, |x| x, u64::cmp);
pattern_test!(a06_patterns_u64_5, "u64", 5, |x| x, u64::cmp);
pattern_test!(a07_patterns_u64_6, "u64", 6, |x| x, u64::cmp);
pattern_test!(a08_patterns_u32_0, "u32", 0, |x| x as u32, u32::cmp);
pattern_test!(a09_patterns_u32_1, "u32", 1, |x| x as u32, u32::cmp);
pattern_test!(a10_patterns_u32_2, "u32", 2, |x| x as u32, u32::cmp);
pattern_test!(a11_patterns_u32_3, "u32", 3, |x| x as u32, u32::cmp);
pattern_test!(a12_patterns_u32_4, "u32", 4, |x| x as u32, u32::cmp);
pattern_test!(a13_patterns_u32_5, "u32", 5, |x| x as u32, u32::cmp);
pattern_test!(a14_patterns_u32_6, "u32", 6, |x| x as u32, u32::cmp);
pattern_test!(a15_patterns_i8_0, "i8", 0, |x| x as i8, i8::cmp);
pattern_test!(a16_patterns_i8_1, "i8", 1, |x| x as i8, i8::cmp);
pattern_test!(a17_patterns_i8_2, "i8", 2, |x| x as i8, i8::cmp);
pattern_test!(a18_patterns_i8_3, "i8", 3, |x| x as i8, i8::cmp);
pattern_test!(a19_patterns_i8_4, "i8", 4, |x| x as i8, i8::cmp);
pattern_test!(a20_patterns_i8_5, "i8", 5, |x| x as i8, i8::cmp);
pattern_test!(a21_patterns_i8_6, "i8", 6, |x| x as i8, i8::cmp);
pattern_test!(a22_patterns_f64_0, "f64", 0, f64::from_bits, f64::total_cmp);
pattern_test!(a23_patterns_f64_1, "f64", 1, f64::from_bits, f64::total_cmp);
pattern_test!(a24_patterns_f64_2, "f64", 2, f64::from_bits, f64::total_cmp);
pattern_test!(a25_patterns_f64_3, "f64", 3, f64::from_bits, f64::total_cmp);
pattern_test!(a26_patterns_f64_4, "f64", 4, f64::from_bits, f64::total_cmp);
pattern_test!(a27_patterns_f64_5, "f64", 5, f64::from_bits, f64::total_cmp);
pattern_test!(a28_patterns_f64_6, "f64", 6, f64::from_bits, f64::total_cmp);
pattern_test!(a29_patterns_string_0, "string", 0, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a30_patterns_string_1, "string", 1, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a31_patterns_string_2, "string", 2, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a32_patterns_string_3, "string", 3, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a33_patterns_string_4, "string", 4, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a34_patterns_string_5, "string", 5, |x| format!("key-{x:016x}"), String::cmp);
pattern_test!(a35_patterns_string_6, "string", 6, |x| format!("key-{x:016x}"), String::cmp);

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    key: u32,
    serial: usize,
    text: String,
}

#[test]
fn a36_stability_keys_and_binary_search() {
    let mut rng = Rng(0xdeadbeef12345678);
    let mut d = D::new();
    for n in (0..=64).chain([65, 513, 4097]) {
        let input: Vec<_> = (0..n)
            .map(|i| Item {
                key: (rng.next() % 17) as u32,
                serial: i,
                text: i.to_string(),
            })
            .collect();
        let mut actual = input.clone();
        actual.sort_by_key(|x| x.key);
        d.obs(&actual);
        let mut cached = input.clone();
        let mut calls = 0;
        cached.sort_by_cached_key(|x| {
            calls += 1;
            x.key
        });
        d.obs(&cached);
        assert_eq!(calls, if n < 2 { 0 } else { n });
        for key in 0..=18 {
            d.obs(&actual.binary_search_by(|x| x.key.cmp(&key)));
        }
    }
    for n in 0..=64 {
        for key in -2..=9 {
            let v: Vec<i32> = (0..n).map(|i| i / 6).collect();
            let mut calls = Vec::new();
            let result = v.binary_search_by(|x| {
                calls.push(*x);
                x.cmp(&key)
            });
            d.obs(&result);
            d.obs(&calls);
        }
    }
    d.line("sort stability and search");
}

#[test]
fn a37_exhaustive_ternary_and_selection_indices() {
    let mut d = D::new();
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
            let mut actual = v.clone();
            actual.sort();
            d.obs(&actual);
            let mut actual = v.clone();
            actual.sort_unstable();
            d.obs(&actual);
            for i in 0..n {
                let mut actual = v.clone();
                let _ = actual.select_nth_unstable(i);
                d.obs(&actual[i]);
                assert!(actual[..i].iter().all(|x| *x <= actual[i]));
                assert!(actual[i + 1..].iter().all(|x| *x >= actual[i]));
            }
        }
    }
    d.line("sort exhaustive ternary");
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

fn unwind_cases<const PAD: usize>(algorithm: usize, pattern: usize) {
    let n = match pattern {
        0 => 19,
        3 => 32,
        _ => 193,
    };
    // tens of thousands of comparison panics: the default hook's report of each is not observed
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut d = D::new();
    for external_touches in [false, true] {
        let mut fail_at = 0;
        let mut outcomes = [false; 2];
        loop {
            let drops = Rc::new((0..n).map(|_| Cell::new(0)).collect::<Vec<_>>());
            let mut rng = Rng(0x87654321);
            let mut v: Vec<_> = (0..n)
                .map(|id| Owned {
                    key: if pattern == 2 {
                        if id < 129 { id as u64 + 1000 } else { id as u64 }
                    } else {
                        rng.next() % 31
                    },
                    id,
                    touches: Cell::new(0),
                    drops: drops.clone(),
                    padding: [0; PAD],
                })
                .collect();
            let touches: Vec<_> = (0..n).map(|_| Cell::new(0usize)).collect();
            let mut calls = 0;
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut cmp = |a: &Owned<PAD>, b: &Owned<PAD>| {
                    if external_touches {
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
                    a.key.cmp(&b.key)
                };
                match algorithm {
                    0 => v.sort_by(&mut cmp),
                    1 => v.sort_unstable_by(&mut cmp),
                    2 => {
                        let _ = v.select_nth_unstable_by(n / 2, &mut cmp);
                    }
                    _ => v.sort_unstable_by(&mut cmp),
                }
            }));
            outcomes[outcome.is_err() as usize] = true;
            assert!(drops.iter().all(|x| x.get() == 0));
            let order: Vec<_> = v.iter().map(|x| x.id).collect();
            let mut ids = order.clone();
            ids.sort();
            assert_eq!(ids, (0..n).collect::<Vec<_>>());
            if external_touches {
                assert_eq!(touches.iter().map(Cell::get).sum::<usize>(), calls * 2);
            } else {
                assert_eq!(v.iter().map(|x| x.touches.get()).sum::<usize>(), calls * 2);
            }
            assert!(v.iter().all(|x| x.padding.iter().all(|&b| b == 0)));
            drop(v);
            assert!(drops.iter().all(|x| x.get() == 1));
            if outcome.is_ok() {
                // Every boundary above checks the ownership properties; the finished run's
                // comparison count and order are the algorithm's, real std's and Relax's alike.
                d.obs(&outcomes);
                d.obs(&calls);
                d.obs(&order);
                d.obs(&drops.iter().map(Cell::get).collect::<Vec<_>>());
                break;
            }
            fail_at += 1;
        }
    }
    std::panic::set_hook(hook);
    d.line(&format!("sort unwind pad {} algorithm {} pattern {}", PAD, algorithm, pattern));
}

macro_rules! unwind_test {
    ($name:ident, $pad:expr, $algorithm:expr, $pattern:expr) => {
        #[test]
        fn $name() {
            unwind_cases::<$pad>($algorithm, $pattern);
        }
    };
}
unwind_test!(a38_panic_pad0_algorithm0_pattern0, 0, 0, 0);
unwind_test!(a39_panic_pad0_algorithm0_pattern1, 0, 0, 1);
unwind_test!(a40_panic_pad0_algorithm0_pattern2, 0, 0, 2);
unwind_test!(a41_panic_pad0_algorithm0_pattern3, 0, 0, 3);
unwind_test!(a42_panic_pad0_algorithm1_pattern0, 0, 1, 0);
unwind_test!(a43_panic_pad0_algorithm1_pattern1, 0, 1, 1);
unwind_test!(a44_panic_pad0_algorithm1_pattern2, 0, 1, 2);
unwind_test!(a45_panic_pad0_algorithm1_pattern3, 0, 1, 3);
unwind_test!(a46_panic_pad0_algorithm2_pattern0, 0, 2, 0);
unwind_test!(a47_panic_pad0_algorithm2_pattern1, 0, 2, 1);
unwind_test!(a48_panic_pad0_algorithm2_pattern2, 0, 2, 2);
unwind_test!(a49_panic_pad0_algorithm2_pattern3, 0, 2, 3);
unwind_test!(a50_panic_pad0_algorithm3_pattern0, 0, 3, 0);
unwind_test!(a51_panic_pad0_algorithm3_pattern1, 0, 3, 1);
unwind_test!(a52_panic_pad0_algorithm3_pattern2, 0, 3, 2);
unwind_test!(a53_panic_pad0_algorithm3_pattern3, 0, 3, 3);
unwind_test!(a54_panic_pad128_algorithm0_pattern0, 128, 0, 0);
unwind_test!(a55_panic_pad128_algorithm0_pattern1, 128, 0, 1);
unwind_test!(a56_panic_pad128_algorithm0_pattern2, 128, 0, 2);
unwind_test!(a57_panic_pad128_algorithm0_pattern3, 128, 0, 3);
unwind_test!(a58_panic_pad128_algorithm1_pattern0, 128, 1, 0);
unwind_test!(a59_panic_pad128_algorithm1_pattern1, 128, 1, 1);
unwind_test!(a60_panic_pad128_algorithm1_pattern2, 128, 1, 2);
unwind_test!(a61_panic_pad128_algorithm1_pattern3, 128, 1, 3);
unwind_test!(a62_panic_pad128_algorithm2_pattern0, 128, 2, 0);
unwind_test!(a63_panic_pad128_algorithm2_pattern1, 128, 2, 1);
unwind_test!(a64_panic_pad128_algorithm2_pattern2, 128, 2, 2);
unwind_test!(a65_panic_pad128_algorithm2_pattern3, 128, 2, 3);
unwind_test!(a66_panic_pad128_algorithm3_pattern0, 128, 3, 0);
unwind_test!(a67_panic_pad128_algorithm3_pattern1, 128, 3, 1);
unwind_test!(a68_panic_pad128_algorithm3_pattern2, 128, 3, 2);
unwind_test!(a69_panic_pad128_algorithm3_pattern3, 128, 3, 3);

#[test]
fn a70_float_bits_zst_and_overaligned() {
    let mut d = D::new();
    let bits = [
        0, 1 << 63, 0x7ff0000000000000, 0xfff0000000000000,
        0x7ff8000000000001, 0xfff8000000000020, 0x7ff0000000000001,
    ];
    let mut v: Vec<f64> = bits.into_iter().cycle().take(777).map(f64::from_bits).collect();
    v.sort_by(f64::total_cmp);
    d.obs(&v.iter().map(|x| x.to_bits()).collect::<Vec<_>>());
    let mut z = [(); 100];
    z.sort_by(|_, _| panic!("ZST comparator"));
    z.sort_unstable_by(|_, _| panic!("ZST comparator"));
    let _ = z.select_nth_unstable_by(50, |_, _| panic!("ZST comparator"));
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        let _ = z.select_nth_unstable_by(100, |_, _| Ordering::Equal);
    }));
    d.obs(&outcome.is_err());
    assert!(outcome.is_err());
    #[repr(align(8192))]
    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Aligned(u64);
    let mut v: Vec<_> = (0..65).map(|x| Aligned((x * 31) % 17)).collect();
    v.sort();
    d.obs(&v);
    d.line("sort floats zst aligned");
}

fn scratch(n: usize) {
    let mut rng = Rng(0xabab01234567);
    let mut v: Vec<_> = (0..n).map(|_| rng.next()).collect();
    v.sort();
    let mut d = D::new();
    // Preserve the full sorted vector without allocating a large Debug string.
    d.bytes(&(v.len() as u64).to_le_bytes());
    for x in v {
        d.bytes(&x.to_le_bytes());
        d.1 += 1;
    }
    d.line(&format!("sort scratch {}", n));
}

#[test]
fn a71_heap_scratch_full() {
    scratch(1_000_001);
}

#[test]
fn a72_heap_scratch_half() {
    scratch(2_000_001);
}

#[test]
fn a73_long_runs() {
    let mut d = D::new();
    for run in [31usize, 64, 129, 511, 2049] {
        let mut v: Vec<_> = (0..100_000).map(|i| ((i / run) % 7, i % run, i)).collect();
        v.sort_by_key(|x| (x.0, x.1));
        d.obs(&v);
    }
    d.line("sort long runs");
}

#[test]
fn a74_copy_struct_keys() {
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
    let mut d = D::new();
    for n in (0..=64).chain([65, 513, 4097]) {
        let input: Vec<_> = (0..n)
            .map(|i| CopyItem {
                key: (rng.next() % 11) as u32,
                value: i as f32,
            })
            .collect();
        let mut actual = input.clone();
        actual.sort_by_key(|x| x.key);
        d.obs(&actual);
        compare(&input, |a, b| a.key.cmp(&b.key), &mut d);
        let wide: Vec<_> = input
            .iter()
            .enumerate()
            .map(|(i, x)| WideItem {
                key: x.key,
                serial: i,
                payload: [i as u64; 4],
            })
            .collect();
        compare(&wide, |a, b| a.key.cmp(&b.key), &mut d);
    }
    d.line("sort copy keys");
}

fn copy_panics(n: usize) {
    let input: Vec<_> = (0..n).map(|i| (((i * 31) % 11) as u32, i as u32)).collect();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let mut d = D::new();
    for stable in [false, true] {
        let mut fail_at = 0;
        let mut outcomes = [false; 2];
        loop {
            let mut v = input.clone();
            let mut calls = 0;
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                let mut cmp = |a: &(u32, u32), b: &(u32, u32)| {
                    calls += 1;
                    if calls == fail_at + 1 {
                        panic!("comparison panic");
                    }
                    a.0.cmp(&b.0)
                };
                if stable {
                    v.sort_by(&mut cmp);
                } else {
                    v.sort_unstable_by(&mut cmp);
                }
            }));
            outcomes[outcome.is_err() as usize] = true;
            let order: Vec<_> = v.iter().map(|x| x.1).collect();
            let mut ids = order.clone();
            ids.sort();
            assert_eq!(ids, (0..n as u32).collect::<Vec<_>>());
            assert!(v.iter().all(|x| x.0 == (x.1 * 31) % 11));
            if outcome.is_ok() {
                d.obs(&outcomes);
                d.obs(&calls);
                d.obs(&order);
                break;
            }
            fail_at += 1;
        }
    }
    std::panic::set_hook(hook);
    d.line(&format!("sort copy panics {}", n));
}

macro_rules! copy_panic_test {
    ($name:ident, $n:expr) => {
        #[test]
        fn $name() {
            copy_panics($n);
        }
    };
}
copy_panic_test!(a75_copy_panics_21, 21);
copy_panic_test!(a76_copy_panics_32, 32);
copy_panic_test!(a77_copy_panics_33, 33);
copy_panic_test!(a78_copy_panics_48, 48);
copy_panic_test!(a79_copy_panics_64, 64);
copy_panic_test!(a80_copy_panics_193, 193);
