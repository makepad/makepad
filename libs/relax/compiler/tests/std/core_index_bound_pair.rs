// Indexing a slice or a str by `(Bound<usize>, Bound<usize>)` (SliceIndex since 1.53): `get` and the
// index over a grid of pairs, past both ends and out of order, with each failure's panic message.
// A str had no such index (E0277), and a slice's failures named other bounds than core's
// (`slice_index_fail`: a start past the length, else an end past it, else the order, else the end).
use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn pairs() -> Vec<(Bound<usize>, Bound<usize>)> {
    let starts = [Unbounded, Included(0), Included(2), Included(5), Included(6), Excluded(0), Excluded(2), Excluded(4), Excluded(5), Excluded(usize::MAX)];
    let ends = [Unbounded, Included(1), Included(4), Included(5), Included(usize::MAX), Excluded(0), Excluded(3), Excluded(5), Excluded(6)];
    starts.iter().flat_map(|s| ends.iter().map(move |e| (*s, *e))).collect()
}

fn outcome(f: impl FnOnce() -> String) -> String {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(e) => format!("panics: {}", e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()),
    }
}

fn slice_case<R: core::slice::SliceIndex<[u16], Output = [u16]> + Copy>(r: R, values: &mut [u16]) {
    let before = values.get(r).unwrap().to_vec();
    values.get_mut(r).unwrap()[0] += 10;
    values[r][0] += 1;
    println!("| slice range {:?} -> {:?}", before, &values[r]);
}

fn string_case<R: core::slice::SliceIndex<str, Output = str> + Copy>(r: R, s: &mut str) {
    let before = s.get(r).unwrap().to_string();
    s.get_mut(r).unwrap().make_ascii_uppercase();
    let upper = s[r].to_string();
    s[r].make_ascii_lowercase();
    println!("| string range {:?} -> {:?} -> {:?}", before, upper, &s[r]);
}

fn hash_of<T: core::hash::Hash>(value: T) -> u64 {
    use core::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn bounds<R: core::ops::RangeBounds<usize>>(r: R) -> (Bound<usize>, Bound<usize>) {
    (r.start_bound().cloned(), r.end_bound().cloned())
}

#[test]
fn a01_slices() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut v = [1u8, 2, 3, 4, 5];
    for p in pairs() {
        let got = v.get(p).map(|s| s.to_vec());
        let at = outcome(|| format!("{:?}", &v[p]));
        let mutated = outcome(|| {
            let s = &mut v[p];
            s.reverse();
            format!("{:?}", s)
        });
        println!("| slice {:?}: get {:?} index {} mut {}", p, got, at, mutated);
        v = [1, 2, 3, 4, 5];
    }
    let half = core::range::Range { start: 1usize, end: 4 };
    let incl = core::range::RangeInclusive { start: 1usize, last: 4 };
    let from = core::range::RangeFrom { start: 1usize };
    let to = core::range::RangeToInclusive { last: 4usize };
    println!("| range debug {:?} {:?} {:?} {:?}", half, incl, from, to);
    let default = std::range::Range::<usize>::default();
    println!("| field hashes {} {} {} {}", hash_of(half) == hash_of((half.start, half.end)),
        hash_of(incl) == hash_of((incl.start, incl.last)), hash_of(from) == hash_of(from.start),
        hash_of(to) == hash_of(to.last));
    println!("| equal {} {} {} {}", half == default, incl == core::range::RangeInclusive::from(1..=4),
        from == core::range::RangeFrom::from(2..), to == core::range::RangeToInclusive::from(..=4));
    println!("| contains {} {} {} {} {} {} {} {}", half.contains(&1), half.contains(&4),
        incl.contains(&4), incl.contains(&5), from.contains(&0), from.contains(&4),
        to.contains(&4), to.contains(&5));
    println!("| empty {} {} {} {} {} {} {}", default.is_empty(), half.is_empty(),
        core::range::Range::from(3..2).is_empty(), incl.is_empty(),
        core::range::RangeInclusive::from(4..=4).is_empty(), core::range::RangeInclusive::from(4..=3).is_empty(),
        core::range::RangeInclusive::from(f64::NAN..=1.0).is_empty());
    println!("| bounds {:?} {:?} {:?} {:?}", bounds(half), bounds(incl), bounds(from), bounds(to));
    let a = 1usize;
    let b = 4usize;
    println!("| ref bounds {:?} {:?} {:?} {:?}", bounds(core::range::Range::from(&a..&b)),
        bounds(core::range::RangeInclusive::from(&a..=&b)), bounds(core::range::RangeFrom::from(&a..)),
        bounds(core::range::RangeToInclusive::from(..=&b)));

    let mut data = [2u16, 3, 5, 7, 11, 13];
    slice_case(half, &mut data);
    slice_case(incl, &mut data);
    slice_case(from, &mut data);
    slice_case(to, &mut data);
    let full: core::range::RangeFull = ..;
    let prefix: core::range::RangeTo<usize> = ..3;
    println!("| range exports {:?} {:?}", &data[full], &data[prefix]);
    println!("| outside slice {:?} {:?} {:?} {:?}", data.get(core::range::Range::from(4..3)),
        data.get(core::range::RangeInclusive::from(0..=usize::MAX)),
        data.get(core::range::RangeFrom::from(7..)), data.get(core::range::RangeToInclusive::from(..=6)));
    println!("| slice range panics {} / {} / {}",
        outcome(|| format!("{:?}", &data[core::range::Range::from(8..7)])),
        outcome(|| format!("{:?}", &mut data[core::range::RangeInclusive::from(0..=6)])),
        outcome(|| format!("{:?}", &data[core::range::RangeToInclusive::from(..=usize::MAX)])));
    // the old inclusive range and the ranges std's collections resolve name the end as it was given
    let mut spent = 5..=5usize;
    spent.next();
    println!("| inclusive {} / {} / {} / {} / {:?} {:?}",
        outcome(|| format!("{:?}", &data[0..=usize::MAX])),
        outcome(|| format!("{:?}", &mut data[2..=6])),
        outcome(|| format!("{:?}", &data[5..=3])),
        outcome(|| format!("{:?}", &data[spent.clone()])),
        data.get(0..=usize::MAX), data.get(1..=5).map(|s| s.len()));
    let mut v = vec![1u8, 2, 3];
    let mut s = String::from("abc");
    let deque = std::collections::VecDeque::from(vec![1u8, 2, 3]);
    println!("| resolved ranges {} / {} / {} / {} / {} / {} / {}",
        outcome(|| format!("{}", v.drain(..=usize::MAX).count())),
        outcome(|| format!("{}", v.drain(..=3).count())),
        outcome(|| format!("{}", v.drain(5..2).count())),
        outcome(|| format!("{}", s.drain(..=3).count())),
        outcome(|| format!("{}", deque.range(..=3).count())),
        outcome(|| { let mut a = [1u8, 2, 3]; a.copy_within(..=3, 0); format!("{a:?}") }),
        outcome(|| { let d: Vec<u8> = v.drain(1..=2).collect(); format!("{d:?} {v:?}") }));
    let parent = &data[1..5];
    println!("| subslice {:?} {:?} {:?} {:?}", parent.subslice_range(&data[2..4]),
        parent.subslice_range(&data[..1]), parent.subslice_range(&data[5..5]),
        parent.subslice_range(&data[2..6]));
    let bytes = [0u8; 9];
    let (triples, _) = bytes.as_chunks::<3>();
    let (shifted, _) = bytes[1..4].as_chunks::<3>();
    println!("| misaligned subslice {:?}", triples.subslice_range(shifted));
    println!("| zero-sized {}", outcome(|| format!("{:?}", [()].subslice_range(&[]))));
}

#[test]
fn a02_strs() {
    std::panic::set_hook(Box::new(|_| {}));
    let s = "hello";
    let mut owned = String::from("hello");
    for p in pairs() {
        let at = outcome(|| s[p].to_string());
        let upper = outcome(|| {
            let part = &mut owned[p];
            part.make_ascii_uppercase();
            part.to_string()
        });
        println!("| str {:?}: get {:?} get_mut {:?} index {} mut {}", p, s.get(p), owned.clone().get_mut(p).map(|x| x.to_string()), at, upper);
        owned = String::from("hello");
    }
    let mut text = String::from("aébc");
    let s = text.as_mut_str();
    string_case(core::range::Range::from(0..3), s);
    string_case(core::range::RangeInclusive::from(1..=3), s);
    string_case(core::range::RangeFrom::from(3..), s);
    string_case(core::range::RangeToInclusive::from(..=3), s);
    let parent = &s[1..4];
    println!("| substr {:?} {:?} {:?} {:?}", parent.substr_range(&s[1..3]), parent.substr_range(&s[0..1]),
        parent.substr_range(&s[4..4]), parent.substr_range(&s[3..5]));
}

#[test]
fn a03_char_boundaries() {
    std::panic::set_hook(Box::new(|_| {}));
    let s = "aé€b";
    for p in [(Included(1), Excluded(3)), (Included(2), Unbounded), (Excluded(0), Included(6)), (Unbounded, Excluded(4))] {
        println!("| {:?}: get {:?} index {}", p, s.get(p), outcome(|| s[p].to_string()));
    }
    println!("| non-boundary {:?} {:?} {:?} {:?}", s.get(core::range::Range::from(0..2)),
        s.get(core::range::RangeInclusive::from(2..=5)), s.get(core::range::RangeFrom::from(4..)),
        s.get(core::range::RangeToInclusive::from(..=1)));
    println!("| outside str {:?}", s.get(core::range::RangeToInclusive::from(..=usize::MAX)));
}
