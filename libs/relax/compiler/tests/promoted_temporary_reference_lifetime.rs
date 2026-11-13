// A shared borrow of a projection of a promotable temporary (an element, a range or a field of an
// array, repeat, tuple or struct literal of constants) borrows the promoted static, as a borrow of
// the whole literal does (Reference destructors.md constant promotion; rustc promote_consts), so
// it outlives the function that makes it. A copy on that function's stack dangled once it returned
// (relax PR #179). A mutable borrow keeps a temporary of its own, an element that is not a constant
// is evaluated each time, and a value with interior mutability is never shared. "| " lines must
// equal rustc's (agree.sh).
use std::cell::Cell;
use std::hint::black_box;

struct Pair {
    a: u32,
    b: u32,
}

struct Outer {
    p: Pair,
    t: (u8, [u16; 3]),
}

#[inline(never)]
fn whole() -> &'static [u32] {
    &[4, 8, 16][..]
}

#[inline(never)]
fn range(lo: usize, hi: usize) -> &'static [u32] {
    &[10, 20, 30, 40][lo..hi]
}

#[inline(never)]
fn element(i: usize) -> &'static u32 {
    &[10, 20, 30, 40][i]
}

#[inline(never)]
fn last_element() -> &'static u32 {
    &[10, 20, 30, 40][3]
}

#[inline(never)]
fn repeat() -> &'static [u64] {
    &[7; 3][..]
}

// a constant whose value has nothing to drop, though its type has a destructor
const NONE: Option<String> = None;

#[inline(never)]
fn nones() -> &'static [Option<String>] {
    &[NONE; 3][1..]
}

#[inline(never)]
fn repeat_param<const N: usize>() -> &'static [usize] {
    &[N; 2][..]
}

#[inline(never)]
fn field() -> &'static u32 {
    &Pair { a: 23, b: 42 }.b
}

#[inline(never)]
fn nested() -> &'static [u16] {
    &Outer { p: Pair { a: 1, b: 2 }, t: (3, [5, 6, 7]) }.t.1[1..]
}

#[inline(never)]
fn tuple_field() -> &'static u8 {
    &(1u8, 2u8, 3u8).2
}

#[inline(never)]
fn row(i: usize) -> &'static [i8] {
    &[[1, -2], [3, -4]][i][..]
}

#[inline(never)]
fn odds() -> std::slice::Iter<'static, u8> {
    [1u8, 3, 5, 7][1..].iter()
}

// Reuses the stack the functions above returned from.
#[inline(never)]
fn clobber() -> u64 {
    let mut v = [0u64; 512];
    for (i, x) in v.iter_mut().enumerate() {
        *x = black_box(i as u64 * 40503 + 1000);
    }
    black_box(&v).iter().sum()
}

#[test]
fn a01_projections_of_constant_literals_outlive_their_function() {
    let whole = black_box(whole as fn() -> &'static [u32])();
    let range = black_box(range as fn(usize, usize) -> &'static [u32])(1, 3);
    let element = black_box(element as fn(usize) -> &'static u32)(2);
    let last = black_box(last_element as fn() -> &'static u32)();
    let repeat = black_box(repeat as fn() -> &'static [u64])();
    let param = black_box(repeat_param::<5> as fn() -> &'static [usize])();
    let nones = black_box(nones as fn() -> &'static [Option<String>])();
    let field = black_box(field as fn() -> &'static u32)();
    let nested = black_box(nested as fn() -> &'static [u16])();
    let tuple_field = black_box(tuple_field as fn() -> &'static u8)();
    let row = black_box(row as fn(usize) -> &'static [i8])(1);
    let odds = black_box(odds as fn() -> std::slice::Iter<'static, u8>)();
    println!("| clobbered {}", clobber() % 7);
    println!("| whole {:?} range {:?} element {} last {}", whole, range, element, last);
    println!("| repeat {:?} const parameter {:?} constants {:?}", repeat, param, nones);
    println!("| field {} nested {:?} tuple field {} row {:?}", field, nested, tuple_field, row);
    println!("| iterator {:?}", odds.copied().collect::<Vec<_>>());
}

#[test]
fn a02_mutable_and_computed_temporaries_stay_per_evaluation() {
    let one = &mut [1, 2, 3][..];
    one[0] = 99;
    let two = &mut [1, 2, 3][..];
    let b = &mut Pair { a: 1, b: 2 }.b;
    *b += 40;
    let again = &Pair { a: 1, b: 2 }.b;
    println!("| mutable {:?} {:?} {} {}", one, two, b, again);
    let mut calls = 0;
    for _ in 0..3 {
        let v = &[
            {
                calls += 1;
                calls
            },
            5,
        ][..];
        let r = &[{
            calls += 10;
            calls
        }; 2][..];
        println!("| computed {:?} {:?}", v, r);
    }
    for _ in 0..2 {
        let c = &[Cell::new(1u8)][0];
        c.set(c.get() + 1);
        println!("| cell {}", c.get());
    }
    let t = &(black_box(7u8), 8u8).0;
    println!("| {} after {} calls", t, calls);
    let o = Outer { p: Pair { a: 3, b: 4 }, t: (9, [1, 2, 3]) };
    println!("| locals {} {} {}", o.p.a + o.p.b, o.t.0, o.t.1[2]);
}
