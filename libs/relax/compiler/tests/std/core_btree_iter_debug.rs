// Debug for the BTreeMap iterators that hand out `&mut V` or owned items: IterMut, IntoIter,
// ValuesMut, IntoKeys, IntoValues (stable since 1.17 / 1.54) and RangeMut. Each prints the items
// it has not yielded yet, as a list. serde_json 1.0's `Map` wraps them in `#[derive(Debug)]`
// structs (src/map.rs: `pub struct IterMut<'a> { iter: IterMutImpl<'a> }`, the same for
// IntoIter, ValuesMut and IntoValues), so the derive needs the impls; the std here had them only
// for Iter, Keys, Values and Range ("expected `&dyn core::fmt::Debug`, found
// `&&IterMut<'_, String, Value>`").
use std::collections::btree_map;
use std::collections::BTreeMap;

// serde_json's shape
#[derive(Debug)]
struct MapIterMut<'a> {
    iter: btree_map::IterMut<'a, String, u32>,
}
#[derive(Debug)]
struct MapIntoIter {
    iter: btree_map::IntoIter<String, u32>,
}
#[derive(Debug)]
struct MapValuesMut<'a> {
    iter: btree_map::ValuesMut<'a, String, u32>,
}
#[derive(Debug)]
struct MapIntoValues {
    iter: btree_map::IntoValues<String, u32>,
}

fn sample() -> BTreeMap<String, u32> {
    let mut m = BTreeMap::new();
    for (i, k) in ["a", "b", "c", "d"].iter().enumerate() {
        m.insert(k.to_string(), i as u32 + 1);
    }
    m
}

#[test]
fn borrowing_iterators_print_what_is_left() {
    let mut m = sample();
    let mut it = m.iter_mut();
    assert_eq!(format!("{:?}", it), "[(\"a\", 1), (\"b\", 2), (\"c\", 3), (\"d\", 4)]");
    *it.next().unwrap().1 += 10;
    it.next_back();
    assert_eq!(format!("{:?}", it), "[(\"b\", 2), (\"c\", 3)]");
    assert_eq!(format!("{:#?}", it), "[\n    (\n        \"b\",\n        2,\n    ),\n    (\n        \"c\",\n        3,\n    ),\n]");
    // printing borrows the iterator only for the call: it still yields `&mut V` afterwards
    *it.next().unwrap().1 = 20;
    it.next();
    assert_eq!(format!("{:?}", it), "[]");
    assert_eq!(it.next().is_none(), true);
    assert_eq!(m["a"], 11);
    assert_eq!(m["b"], 20);

    let mut vals = m.values_mut();
    assert_eq!(format!("{:?}", vals), "[11, 20, 3, 4]");
    vals.next();
    vals.next_back();
    assert_eq!(format!("{:x?}", vals), "[14, 3]");
    assert_eq!(format!("{:?}", MapValuesMut { iter: m.values_mut() }), "MapValuesMut { iter: [11, 20, 3, 4] }");
    assert_eq!(
        format!("{:?}", MapIterMut { iter: m.iter_mut() }),
        "MapIterMut { iter: [(\"a\", 11), (\"b\", 20), (\"c\", 3), (\"d\", 4)] }"
    );

    let mut r = m.range_mut("b".to_string().."d".to_string());
    assert_eq!(format!("{:?}", r), "[(\"b\", 20), (\"c\", 3)]");
    *r.next().unwrap().1 = 2;
    assert_eq!(format!("{:?}", r), "[(\"c\", 3)]");
    r.next_back();
    assert_eq!(format!("{:?}", r), "[]");
    assert_eq!(format!("{:?}", m.range_mut("x".to_string()..)), "[]");
    assert_eq!(m["b"], 2);

    let mut empty: BTreeMap<u8, u8> = BTreeMap::new();
    assert_eq!(format!("{:?}", empty.iter_mut()), "[]");
    assert_eq!(format!("{:?}", empty.values_mut()), "[]");
}

#[test]
fn owning_iterators_print_what_is_left() {
    let mut it = sample().into_iter();
    assert_eq!(format!("{:?}", it), "[(\"a\", 1), (\"b\", 2), (\"c\", 3), (\"d\", 4)]");
    assert_eq!(it.next(), Some(("a".to_string(), 1)));
    assert_eq!(it.next_back(), Some(("d".to_string(), 4)));
    assert_eq!(format!("{:?}", it), "[(\"b\", 2), (\"c\", 3)]");
    assert_eq!(format!("{:#?}", it), "[\n    (\n        \"b\",\n        2,\n    ),\n    (\n        \"c\",\n        3,\n    ),\n]");
    assert_eq!(it.len(), 2);
    it.next();
    it.next();
    assert_eq!(format!("{:?}", it), "[]");

    let mut keys = sample().into_keys();
    assert_eq!(format!("{:?}", keys), "[\"a\", \"b\", \"c\", \"d\"]");
    keys.next();
    assert_eq!(format!("{:?}", keys), "[\"b\", \"c\", \"d\"]");
    assert_eq!(keys.next_back().as_deref(), Some("d"));

    let mut vals = sample().into_values();
    assert_eq!(format!("{:?}", vals), "[1, 2, 3, 4]");
    vals.next_back();
    vals.next_back();
    assert_eq!(format!("{:#?}", vals), "[\n    1,\n    2,\n]");
    assert_eq!(vals.next(), Some(1));

    assert_eq!(
        format!("{:?}", MapIntoIter { iter: sample().into_iter() }),
        "MapIntoIter { iter: [(\"a\", 1), (\"b\", 2), (\"c\", 3), (\"d\", 4)] }"
    );
    assert_eq!(format!("{:?}", MapIntoValues { iter: sample().into_values() }), "MapIntoValues { iter: [1, 2, 3, 4] }");
    assert_eq!(format!("{:?}", BTreeMap::<u8, u8>::new().into_iter()), "[]");
    assert_eq!(format!("{:?}", BTreeMap::<u8, u8>::new().into_keys()), "[]");
    assert_eq!(format!("{:?}", BTreeMap::<u8, u8>::new().into_values()), "[]");
}
