// Debug for the HashMap iterators that hand out `&mut V` or owned items: IterMut, IntoIter, Drain
// (stable since 1.16), ValuesMut, IntoKeys, IntoValues and ExtractIf (1.88). The first six print
// the entries they have not yielded yet as a list; ExtractIf prints `ExtractIf { .. }`. slotmap
// 1.0's SparseSecondaryMap wraps `hash_map::{Drain, IntoIter, IterMut}` in `#[derive(Debug)]`
// structs (src/sparse_secondary.rs), so the derive needs the impls; the std here had them only
// for Iter, Keys and Values ("expected `&dyn core::fmt::Debug`, found `&&Drain<'_, u32, Slot<V>>`").
use std::collections::hash_map;
use std::collections::HashMap;

// slotmap's shape
#[derive(Debug)]
struct Slot<T> {
    version: u32,
    value: T,
}
#[derive(Debug)]
struct SparseDrain<'a, V: 'a> {
    inner: hash_map::Drain<'a, u32, Slot<V>>,
}
#[derive(Debug)]
struct SparseIntoIter<V> {
    inner: hash_map::IntoIter<u32, Slot<V>>,
}
#[derive(Debug)]
struct SparseIterMut<'a, V: 'a> {
    inner: hash_map::IterMut<'a, u32, Slot<V>>,
}

fn sample() -> HashMap<u32, u32> {
    let mut m = HashMap::new();
    for i in 1..=4u32 {
        m.insert(i, i * 10);
    }
    m
}

fn one() -> HashMap<u32, Slot<&'static str>> {
    let mut m = HashMap::new();
    m.insert(7, Slot { version: 3, value: "seven" });
    m
}

// A hash map's order is unspecified: compare the printed entries as a sorted list.
fn sorted(printed: &str, open: &str, sep: &str, close: &str) -> Vec<String> {
    let inner = printed.strip_prefix("[").unwrap().strip_suffix("]").unwrap();
    if inner.is_empty() {
        return Vec::new();
    }
    let inner = inner.strip_prefix(open).unwrap().strip_suffix(close).unwrap();
    let mut v: Vec<String> = inner.split(sep).map(|p| p.to_string()).collect();
    v.sort();
    v
}
fn pairs(printed: &str) -> Vec<String> {
    sorted(printed, "(", "), (", ")")
}
fn items(printed: &str) -> Vec<String> {
    sorted(printed, "", ", ", "")
}

#[test]
fn borrowing_iterators_print_what_is_left() {
    let mut m = sample();
    let mut it = m.iter_mut();
    assert_eq!(pairs(&format!("{:?}", it)), ["1, 10", "2, 20", "3, 30", "4, 40"]);
    let (k, v) = it.next().unwrap();
    *v += 1;
    let gone = format!("{}, {}", k, *v - 1);
    let left = pairs(&format!("{:?}", it));
    assert_eq!(left.len(), 3);
    assert_eq!(left.contains(&gone), false);
    // printing borrows the iterator only for the call: it still yields `&mut V` afterwards
    for (_, v) in &mut it {
        *v += 2;
    }
    assert_eq!(format!("{:?}", it), "[]");
    let mut after: Vec<u32> = m.values().copied().collect();
    after.sort();
    assert_eq!(after.iter().sum::<u32>(), 100 + 1 + 6);

    let mut vals = m.values_mut();
    assert_eq!(items(&format!("{:?}", vals)).len(), 4);
    vals.next();
    vals.next();
    assert_eq!(items(&format!("{:?}", vals)).len(), 2);

    let mut single = one();
    assert_eq!(format!("{:?}", single.iter_mut()), "[(7, Slot { version: 3, value: \"seven\" })]");
    assert_eq!(
        format!("{:#?}", single.iter_mut()),
        "[\n    (\n        7,\n        Slot {\n            version: 3,\n            value: \"seven\",\n        },\n    ),\n]"
    );
    assert_eq!(format!("{:?}", single.values_mut()), "[Slot { version: 3, value: \"seven\" }]");
    assert_eq!(
        format!("{:?}", SparseIterMut { inner: single.iter_mut() }),
        "SparseIterMut { inner: [(7, Slot { version: 3, value: \"seven\" })] }"
    );
    let mut empty: HashMap<u8, u8> = HashMap::new();
    assert_eq!(format!("{:?}", empty.iter_mut()), "[]");
    assert_eq!(format!("{:?}", empty.values_mut()), "[]");
}

#[test]
fn owning_iterators_print_what_is_left() {
    let mut it = sample().into_iter();
    assert_eq!(pairs(&format!("{:?}", it)), ["1, 10", "2, 20", "3, 30", "4, 40"]);
    let (k, v) = it.next().unwrap();
    let left = pairs(&format!("{:?}", it));
    assert_eq!(left.len(), 3);
    assert_eq!(left.contains(&format!("{}, {}", k, v)), false);
    assert_eq!(it.len(), 3);
    assert_eq!(it.by_ref().count(), 3);
    assert_eq!(format!("{:?}", it), "[]");

    let mut keys = sample().into_keys();
    assert_eq!(items(&format!("{:?}", keys)), ["1", "2", "3", "4"]);
    let k = keys.next().unwrap();
    let left = items(&format!("{:?}", keys));
    assert_eq!(left.len(), 3);
    assert_eq!(left.contains(&k.to_string()), false);

    let mut vals = sample().into_values();
    assert_eq!(items(&format!("{:x?}", vals)), ["14", "1e", "28", "a"]);
    let v = vals.next().unwrap();
    let left = items(&format!("{:?}", vals));
    assert_eq!(left.len(), 3);
    assert_eq!(left.contains(&v.to_string()), false);

    assert_eq!(format!("{:?}", one().into_iter()), "[(7, Slot { version: 3, value: \"seven\" })]");
    assert_eq!(format!("{:?}", one().into_keys()), "[7]");
    assert_eq!(format!("{:#?}", one().into_keys()), "[\n    7,\n]");
    assert_eq!(format!("{:?}", one().into_values()), "[Slot { version: 3, value: \"seven\" }]");
    assert_eq!(
        format!("{:?}", SparseIntoIter { inner: one().into_iter() }),
        "SparseIntoIter { inner: [(7, Slot { version: 3, value: \"seven\" })] }"
    );
    assert_eq!(format!("{:?}", HashMap::<u8, u8>::new().into_iter()), "[]");
    assert_eq!(format!("{:?}", HashMap::<u8, u8>::new().into_keys()), "[]");
    assert_eq!(format!("{:?}", HashMap::<u8, u8>::new().into_values()), "[]");
}

#[test]
fn drain_and_extract_if() {
    let mut m = sample();
    {
        let mut d = m.drain();
        assert_eq!(pairs(&format!("{:?}", d)), ["1, 10", "2, 20", "3, 30", "4, 40"]);
        let (k, v) = d.next().unwrap();
        let left = pairs(&format!("{:?}", d));
        assert_eq!(left.len(), 3);
        assert_eq!(left.contains(&format!("{}, {}", k, v)), false);
        assert_eq!(d.len(), 3);
    }
    assert_eq!(m.len(), 0);
    assert_eq!(format!("{:?}", m.drain()), "[]");

    let mut single = one();
    assert_eq!(
        format!("{:?}", SparseDrain { inner: single.drain() }),
        "SparseDrain { inner: [(7, Slot { version: 3, value: \"seven\" })] }"
    );
    assert_eq!(single.is_empty(), true);
    let mut single = one();
    let mut d = single.drain();
    assert_eq!(format!("{:#?}", d).starts_with("[\n    (\n        7,\n        Slot {\n"), true);
    assert_eq!(d.next().map(|(k, s)| (k, s.version, s.value)), Some((7, 3, "seven")));
    assert_eq!(format!("{:?}", d), "[]");
    drop(d);

    let mut m = sample();
    let mut e = m.extract_if(|k, _| *k % 2 == 0);
    assert_eq!(format!("{:?}", e), "ExtractIf { .. }");
    assert_eq!(format!("{:#?}", e), "ExtractIf { .. }");
    let mut taken: Vec<(u32, u32)> = Vec::new();
    taken.push(e.next().unwrap());
    assert_eq!(format!("{:?}", e), "ExtractIf { .. }");
    taken.extend(e);
    taken.sort();
    assert_eq!(taken, [(2, 20), (4, 40)]);
    assert_eq!(m.len(), 2);
}
