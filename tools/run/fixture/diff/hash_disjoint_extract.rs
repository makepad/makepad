// diff_fast corpus program: disjoint mutable map lookup (including the duplicate-key panic),
// lazy set extraction and early drop, fallible reservation, and collection unwind-safety bounds.
// rustc --test -O and Relax must print identical D lines; hash collections are sorted for output.
use std::collections::{hash_set::ExtractIf, HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};

macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

fn ref_unwind_safe<T: std::panic::RefUnwindSafe>() {}
fn unwind_safe<T: std::panic::UnwindSafe>() {}
fn fused<I: std::iter::FusedIterator>(_: &I) {}
fn extract_without_bounds<T, S>(set: &mut HashSet<T, S>) {
    let mut iter: ExtractIf<'_, T, _> = set.extract_if(|_| true);
    fused(&iter);
    d!("unbounded empty {}", iter.next().is_none());
}
fn sorted(set: &HashSet<i32>) -> Vec<i32> {
    let mut values: Vec<_> = set.iter().copied().collect();
    values.sort();
    values
}

fn work() {
    ref_unwind_safe::<HashMap<i32, &'static mut i32>>();
    ref_unwind_safe::<HashSet<i32>>();
    unwind_safe::<HashMap<i32, String>>();
    unwind_safe::<HashSet<i32>>();
    // UnwindSafe must not additionally require RefUnwindSafe of any type parameter.
    unwind_safe::<HashMap<std::cell::Cell<i32>, std::cell::Cell<i32>, std::cell::Cell<i32>>>();
    unwind_safe::<HashSet<std::cell::Cell<i32>, std::cell::Cell<i32>>>();
    d!("unwind bounds ok");

    let mut map = HashMap::from([(String::from("one"), 10), (String::from("two"), 20)]);
    let [a, b] = map.get_disjoint_mut(["one", "two"]);
    *a.unwrap() += 1;
    *b.unwrap() += 2;
    d!("present {} {}", map["one"], map["two"]);
    let [a, b] = map.get_disjoint_mut(["one", "missing"]);
    *a.unwrap() += 100;
    d!("missing {}", b.is_none());
    d!("zero {}", map.get_disjoint_mut::<str, 0>([]).len());
    let [a, b] = map.get_disjoint_mut(["missing", "missing"]);
    d!("duplicate missing {} {}", a.is_none(), b.is_none());

    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = map.get_disjoint_mut(["one", "one"]);
    }));
    std::panic::set_hook(hook);
    let payload = result.unwrap_err();
    let message = if let Some(s) = payload.downcast_ref::<&str>() {
        *s
    } else {
        payload.downcast_ref::<String>().unwrap().as_str()
    };
    d!("duplicate panic {}", message);
    assert_eq!(message, "duplicate keys found");

    let [a, b] = unsafe { map.get_disjoint_unchecked_mut(["two", "one"]) };
    *a.unwrap() *= 2;
    *b.unwrap() *= 3;
    let mut entries: Vec<_> = map.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    entries.sort();
    d!("unchecked {:?}", entries);
    let mut taken: Vec<_> = map.extract_if(|k, v| { *v += 1; k == "two" }).collect();
    taken.sort();
    let mut kept: Vec<_> = map.into_iter().collect();
    kept.sort();
    d!("map extract {:?} kept {:?}", taken, kept);

    let mut set: HashSet<_> = (1..=6).collect();
    let mut iter: ExtractIf<'_, i32, _> = set.extract_if(|v| v % 2 == 0);
    fused(&iter);
    d!("extract {:?} hint {:?}", iter, iter.size_hint());
    let mut taken = Vec::new();
    for value in iter.by_ref() { taken.push(value); }
    d!("exhausted {:?} {:?} hint {:?}", iter.next(), iter.next(), iter.size_hint());
    drop(iter);
    taken.sort();
    d!("evens {:?} kept {:?}", taken, sorted(&set));

    let mut panic_set = HashSet::from([7]);
    let mut iter = panic_set.extract_if(|_| -> bool { panic!("predicate panic") });
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_unwind(AssertUnwindSafe(|| iter.next()));
    std::panic::set_hook(hook);
    d!("predicate panicked {} next {:?} hint {:?}", result.is_err(), iter.next(), iter.size_hint());
    drop(iter);
    d!("predicate kept {:?}", sorted(&panic_set));

    let mut set: HashSet<_> = (1..=6).collect();
    let first;
    let mut calls = 0;
    {
        let mut iter = set.extract_if(|_| { calls += 1; true });
        first = iter.next().unwrap();
    }
    // The first key is arbitrary. Compare membership/counts, then restore it for sorted output.
    d!("partial calls {} len {} removed {}", calls, set.len(), !set.contains(&first));
    set.insert(first);
    d!("partial restored {:?}", sorted(&set));
    let before = set.len();
    d!("reserve ok {}", set.try_reserve(16).is_ok());
    d!("reserve room {} kept {:?}", set.capacity() >= before + 16, sorted(&set));
    let error = set.try_reserve(usize::MAX).unwrap_err();
    d!("reserve overflow {} kept {:?}", error, sorted(&set));
    let mut empty: HashSet<i32> = HashSet::new();
    d!("empty overflow {}", empty.try_reserve(usize::MAX).unwrap_err());

    struct NoTraits;
    extract_without_bounds(&mut HashSet::<NoTraits, ()>::with_hasher(()));
}
