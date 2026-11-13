// `==`, `!=`, `<` and the rest on an opaque type (`impl PartialEq` / `impl PartialOrd` another fn
// returns) call its bound's method, whatever its hidden type: rustc builds in a comparison of scalar
// operands only (op.rs is_builtin_binop). A built-in comparison of an aggregate hidden type found two
// different values equal (an app's `structure(&before) != structure(&after)` over a tuple of maps).
use std::collections::BTreeMap;
use std::fmt::Debug;

fn array(k: bool) -> impl PartialEq + Debug {
    [k as u64, 7]
}

fn maps(k: bool) -> impl PartialEq {
    let mut m = BTreeMap::new();
    if k {
        m.insert(String::from("theme"), String::from("dark"));
    }
    (m, vec![String::from("a")], vec![1.5f32])
}

fn ordered(k: u32) -> impl PartialOrd {
    (k, String::from("x"))
}

fn scalar(k: u64) -> impl PartialEq + PartialOrd {
    k
}

#[derive(PartialEq, PartialOrd)]
struct Point {
    x: i32,
    y: i32,
}

fn point(x: i32) -> impl PartialEq + PartialOrd {
    Point { x, y: -x }
}

#[test]
fn opaque_operands_compare_by_value() {
    assert!(array(false) != array(true), "an array");
    assert!(!(array(false) == array(true)) && array(true) == array(true), "an array, both ways");
    assert!(maps(false) != maps(true) && maps(true) == maps(true), "a tuple of maps");
    assert!(ordered(1) < ordered(2) && !(ordered(2) <= ordered(1)) && ordered(3) >= ordered(3), "a tuple's order");
    assert!(scalar(0) != scalar(1) && scalar(2) > scalar(1), "a scalar");
    assert!(point(1) != point(2) && point(1) < point(2) && point(2) == point(2), "a struct");
}
