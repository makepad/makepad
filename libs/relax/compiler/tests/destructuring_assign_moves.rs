// Destructuring assignment moves its parts out of a place: each value is dropped once, by its new
// owner, and the source keeps (and drops) only what the assignee left in it. Array assignees from
// an array local, into array, Vec and struct-field places; tuple, struct and nested sources.
// Differential file: `| ` lines must match under rustc --test and makepad test (drop order
// included). An array source used to be dropped again at its scope's end: a double free.

use std::cell::RefCell;
use std::ops::Range;

thread_local! {
    static LOG: RefCell<Vec<&'static str>> = RefCell::new(Vec::new());
}

struct N(&'static str);

impl Drop for N {
    fn drop(&mut self) {
        LOG.with(|l| l.borrow_mut().push(self.0));
    }
}

/// The drops since the last line, in order.
fn line(tag: &str) {
    let log = LOG.with(|l| std::mem::take(&mut *l.borrow_mut()));
    println!("| {tag}: {}", log.join(" "));
}

struct Pair {
    a: N,
    b: N,
}

struct Holder {
    pair: [N; 2],
    tail: N,
}

fn sets() -> ([Vec<Range<u32>>; 2], usize, usize) {
    ([vec![0..2, 4..6], vec![8..9]], 1, 2)
}

/// Into array index places from an array local bound by a tuple pattern (the shape that freed a
/// buffer twice), then each slot taken out inside a closure.
#[test]
fn a01_array_local_into_index_places() {
    const SET: usize = 3;
    let mut near: [Vec<Range<u32>>; 4] = Default::default();
    let mut far: [Vec<Range<u32>>; 4] = [vec![1..2], vec![], vec![], vec![3..4]];
    let (set_ranges, went, came) = sets();
    [near[SET], far[SET]] = set_ranges;
    let taken: [[Vec<Range<u32>>; 2]; 4] = std::array::from_fn(|s| [std::mem::take(&mut near[s]), std::mem::take(&mut far[s])]);
    println!("| a01 ranges: {taken:?} {went} {came}");
    let mut olds = [N("o0"), N("o1")];
    let mut fars = [N("p0"), N("p1")];
    {
        let set = [N("s0"), N("s1")];
        [olds[1], fars[0]] = set;
        line("a01 assigned");
    }
    line("a01 set's scope");
    drop(olds);
    line("a01 olds");
    drop(fars);
    line("a01 fars");
}

/// `_` and `..` leave their elements in the source local, which drops them at its scope's end.
#[test]
fn a02_unmatched_elements_stay_with_the_source() {
    let (a, b);
    {
        let arr = [N("e0"), N("e1"), N("e2")];
        [a, _, ..] = arr;
        line("a02 [a, _, ..]");
    }
    line("a02 arr's scope");
    {
        let arr = [N("g0"), N("g1"), N("g2")];
        [.., b] = arr;
        line("a02 [.., b]");
    }
    line("a02 arr's scope");
    drop(a);
    drop(b);
    line("a02 a, b");
}

/// Into struct fields and Vec elements, from an array local and from a tuple local.
#[test]
fn a03_into_fields_and_vec_elements() {
    let mut p = Pair { a: N("pa"), b: N("pb") };
    let mut v = vec![N("v0"), N("v1")];
    {
        let arr = [N("x0"), N("x1")];
        [p.a, v[1]] = arr;
        line("a03 [p.a, v[1]]");
        let tup = (N("t0"), N("t1"));
        (v[0], p.b) = tup;
        line("a03 (v[0], p.b)");
    }
    line("a03 sources' scope");
    drop(p);
    line("a03 p");
    drop(v);
    line("a03 v");
}

/// Arrays inside a tuple, a struct field and another array: the moved elements' paths go through
/// the containing parts.
#[test]
fn a04_nested_sources() {
    let (w, x, y, z);
    {
        let tup = (N("q0"), [N("q1"), N("q2")]);
        (w, [x, _]) = tup;
        line("a04 (w, [x, _])");
    }
    line("a04 tup's scope");
    {
        let h = Holder { pair: [N("h0"), N("h1")], tail: N("ht") };
        [_, y] = h.pair;
        line("a04 [_, y] = h.pair");
        println!("| a04 h.tail: {}", h.tail.0);
    }
    line("a04 h's scope");
    {
        let grid = [[N("r0"), N("r1")], [N("r2"), N("r3")]];
        [[_, z], _] = grid;
        line("a04 [[_, z], _]");
    }
    line("a04 grid's scope");
    drop((w, x, y, z));
    line("a04 w, x, y, z");
}

/// Strings out of an array local; struct and tuple-struct assignees from locals; an array local
/// given a new value after its elements moved out drops only the new one.
#[test]
fn a05_strings_structs_and_reassignment() {
    let words = [String::from("left"), String::from("right")];
    let (mut l, mut r): (String, String);
    [l, r] = words;
    l.push('!');
    r.push('?');
    println!("| a05 strings: {l} {r}");
    let (c, d);
    {
        let pair = Pair { a: N("sa"), b: N("sb") };
        Pair { a: c, b: d } = pair;
        line("a05 Pair { a: c, b: d }");
    }
    line("a05 pair's scope");
    drop(d);
    drop(c);
    line("a05 d, c");
    let (e, f);
    {
        let mut arr = [N("m0"), N("m1")];
        [e, f] = arr;
        line("a05 [e, f]");
        arr = [N("n0"), N("n1")];
        line("a05 arr given a new value");
        println!("| a05 arr: {} {}", arr[0].0, arr[1].0);
    }
    line("a05 arr's scope");
    drop((e, f));
    line("a05 e, f");
}

/// A temporary on the right drops the elements the assignee leaves at the statement.
#[test]
fn a06_temporary_sources() {
    let (a, b);
    [a, _] = [N("k0"), N("k1")];
    line("a06 [a, _] = [..]");
    [.., b] = [N("k2"), N("k3"), N("k4")];
    line("a06 [.., b] = [..]");
    drop((a, b));
    line("a06 a, b");
}
