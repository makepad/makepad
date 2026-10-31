// Match lowering as a decision tree (`makepad test`; rustc: every test passes). Each case is a
// match whose arms the tree dispatches (three or more arms over integers, chars, bools, enum
// variants, tuples of them, references, or-patterns, ranges): the arm taken, the order guards run
// in (and run again after a failed guard), bindings, and the scrutinee being read once.
use std::cell::RefCell;

thread_local! { static LOG: RefCell<Vec<u32>> = RefCell::new(Vec::new()); }
fn log(x: u32) -> bool { LOG.with(|l| l.borrow_mut().push(x)); true }
fn nolog(x: u32) -> bool { LOG.with(|l| l.borrow_mut().push(x)); false }
fn take() -> Vec<u32> { LOG.with(|l| std::mem::take(&mut *l.borrow_mut())) }

fn ints(x: i32) -> u32 {
    match x {
        i32::MIN..=-100 => 1,
        -99..=-1 => 2,
        0 => 3,
        1 | 2 | 3 => 4,
        4..=9 | 20..=29 => 5,
        10..=19 => 6,
        100 => 7,
        _ => 8,
    }
}
#[test] fn integer_ranges() {
    let want = |x: i32| -> u32 {
        if x <= -100 { 1 } else if x < 0 { 2 } else if x == 0 { 3 } else if x <= 3 { 4 } else if x <= 9 { 5 } else if x <= 19 { 6 }
        else if x <= 29 { 5 } else if x == 100 { 7 } else { 8 }
    };
    for x in (-300..300).chain([i32::MIN, i32::MAX, i32::MIN + 1, i32::MAX - 1]) {
        assert_eq!(ints(x), want(x), "{x}");
    }
}

fn bytes(c: u8) -> u32 {
    match c {
        b'a'..=b'z' | b'A'..=b'Z' | b'_' => 1,
        b'0'..=b'9' => 2,
        b' ' | b'\n' | b'\t' | b'\r' => 3,
        b'"' => 4,
        b'/' => 5,
        0x80..=0xff => 6,
        _ => 7,
    }
}
#[test] fn byte_classes() {
    for c in 0..=255u8 {
        let want = if c.is_ascii_alphabetic() || c == b'_' { 1 } else if c.is_ascii_digit() { 2 } else if matches!(c, b' ' | b'\n' | b'\t' | b'\r') { 3 }
            else if c == b'"' { 4 } else if c == b'/' { 5 } else if c >= 0x80 { 6 } else { 7 };
        assert_eq!(bytes(c), want, "{c}");
    }
}

fn wide(x: u64) -> u32 {
    match x {
        0 => 1,
        1..=0xffff_ffff => 2,
        0x1_0000_0000..=0x7fff_ffff_ffff_ffff => 3,
        0x8000_0000_0000_0000..=0xffff_ffff_ffff_fffe => 4,
        u64::MAX => 5,
    }
}
fn signed8(x: i8) -> u32 {
    match x {
        i8::MIN => 1,
        -127..=-1 => 2,
        0 => 3,
        1..=126 => 4,
        i8::MAX => 5,
    }
}
#[test] fn extreme_values() {
    for (x, w) in [(0u64, 1), (1, 2), (0xffff_ffff, 2), (0x1_0000_0000, 3), (0x7fff_ffff_ffff_ffff, 3), (0x8000_0000_0000_0000, 4), (u64::MAX - 1, 4), (u64::MAX, 5)] {
        assert_eq!(wide(x), w, "{x}");
    }
    for x in i8::MIN..=i8::MAX {
        let w = if x == i8::MIN { 1 } else if x < 0 { 2 } else if x == 0 { 3 } else if x < 127 { 4 } else { 5 };
        assert_eq!(signed8(x), w, "{x}");
    }
}

fn chars(c: char) -> u32 {
    match c {
        'a'..='z' => 1,
        'A'..='Z' => 2,
        '0'..='9' | '_' => 3,
        '\u{80}'..='\u{10ffff}' => 4,
        '\0' => 5,
        _ => 6,
    }
}
#[test] fn char_ranges() {
    for (c, w) in [('a', 1), ('z', 1), ('A', 2), ('_', 3), ('5', 3), ('é', 4), ('\u{10ffff}', 4), ('\0', 5), ('!', 6), ('{', 6)] {
        assert_eq!(chars(c), w, "{c:?}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum E { A, B(i32), C { x: u8, y: bool }, D(Option<u8>), F }

fn en(e: E) -> i64 {
    match e {
        E::A => 1,
        E::B(n) if n < 0 => -1,
        E::B(0) => 2,
        E::B(n) => 100 + n as i64,
        E::C { x: 0, y } => 200 + y as i64,
        E::C { x, y: true } => 300 + x as i64,
        E::C { .. } => 400,
        E::D(None) => 5,
        E::D(Some(v @ 10..=20)) => 500 + v as i64,
        E::D(Some(_)) => 6,
        E::F => 7,
    }
}
#[test] fn enum_variants() {
    let cases = [
        (E::A, 1), (E::B(-5), -1), (E::B(0), 2), (E::B(7), 107), (E::C { x: 0, y: false }, 200), (E::C { x: 0, y: true }, 201),
        (E::C { x: 9, y: true }, 309), (E::C { x: 9, y: false }, 400), (E::D(None), 5), (E::D(Some(15)), 515), (E::D(Some(30)), 6), (E::F, 7),
    ];
    for (e, w) in cases {
        assert_eq!(en(e), w, "{e:?}");
    }
}

fn pairs(a: Option<i32>, b: Result<u8, char>) -> u32 {
    match (a, b) {
        (None, Ok(0)) => 1,
        (None, Ok(_)) => 2,
        (None, Err('x')) => 3,
        (Some(0), _) => 4,
        (Some(n), Ok(m)) if n as i64 == m as i64 => 5,
        (Some(1..=9), Err(_)) => 6,
        (Some(_), Ok(1 | 2)) => 7,
        (_, Err('a'..='f')) => 8,
        _ => 9,
    }
}
#[test] fn tuple_scrutinee() {
    let cases = [
        (None, Ok(0), 1), (None, Ok(9), 2), (None, Err('x'), 3), (Some(0), Ok(3), 4), (Some(0), Err('q'), 4), (Some(4), Ok(4), 5),
        (Some(5), Err('z'), 6), (Some(50), Ok(2), 7), (Some(50), Err('c'), 8), (None, Err('c'), 8), (Some(50), Ok(51), 9), (Some(-5), Err('z'), 9),
    ];
    for (a, b, w) in cases {
        assert_eq!(pairs(a, b), w, "{a:?} {b:?}");
    }
}

// guards run in arm order, once per arm, and a failed guard goes on with the arms after it
fn guards(x: i32) -> u32 {
    match x {
        n if nolog(1) && n > 0 => 1,
        1 | 2 if nolog(2) => 2,
        1 | 3 if log(3) => 3,
        0..=5 if nolog(4) => 4,
        _ if log(5) => 5,
        _ => 6,
    }
}
#[test] fn guard_order() {
    take();
    assert_eq!(guards(1), 3); assert_eq!(take(), vec![1, 2, 3]);
    assert_eq!(guards(2), 5); assert_eq!(take(), vec![1, 2, 4, 5]);
    assert_eq!(guards(3), 3); assert_eq!(take(), vec![1, 3]);
    assert_eq!(guards(-7), 5); assert_eq!(take(), vec![1, 5]);
}

// the scrutinee expression runs once
fn counted(n: &mut u32, x: i32) -> i32 { *n += 1; x }
fn once(x: i32) -> (u32, u32) {
    let mut n = 0;
    let r = match counted(&mut n, x) { 0 => 10, 1..=4 => 11, 5 | 6 => 12, 7..=100 => 13, _ => 14 };
    (n, r)
}
#[test] fn scrutinee_once() {
    for (x, r) in [(0, 10), (3, 11), (6, 12), (50, 13), (-1, 14), (1000, 14)] {
        assert_eq!(once(x), (1, r));
    }
}

// references: match ergonomics peel them, the pointee is read once
fn refs(o: &Option<(i32, &str)>) -> u32 {
    match o {
        None => 1,
        Some((0, _)) => 2,
        Some((n, "x")) if *n > 5 => 3,
        Some((_, "x")) => 4,
        Some((1..=3, s)) if s.len() == 2 => 5,
        Some(_) => 6,
    }
}
#[test] fn through_references() {
    assert_eq!(refs(&None), 1);
    assert_eq!(refs(&Some((0, "q"))), 2);
    assert_eq!(refs(&Some((9, "x"))), 3);
    assert_eq!(refs(&Some((2, "x"))), 4);
    assert_eq!(refs(&Some((2, "ab"))), 5);
    assert_eq!(refs(&Some((2, "abc"))), 6);
    assert_eq!(refs(&Some((7, "ab"))), 6);
}

// bindings: by value, by reference, @ patterns, or-patterns that bind, nested
fn binds(v: &(Option<u32>, u32)) -> u32 {
    match v {
        (Some(a @ (1 | 2)), b) => a * 10 + b,
        (Some(a), 0) => *a + 1000,
        (Some(a @ 3..=8), b) | (None, a @ (b @ 3..=8)) => *a + *b + 2000,
        (None, c) => *c + 3000,
        (Some(_), b) => *b,
    }
}
#[test] fn bindings() {
    assert_eq!(binds(&(Some(1), 5)), 15);
    assert_eq!(binds(&(Some(2), 0)), 20);
    assert_eq!(binds(&(Some(40), 0)), 1040);
    assert_eq!(binds(&(Some(4), 9)), 2013);
    assert_eq!(binds(&(None, 5)), 2010);
    assert_eq!(binds(&(None, 30)), 3030);
    assert_eq!(binds(&(Some(40), 9)), 9);
}

// drops: the moved-out parts and the rest drop where they did
struct D(u32);
impl Drop for D { fn drop(&mut self) { log(900 + self.0); } }
fn drops(e: Result<D, (D, D)>, k: u32) -> u32 {
    match e {
        Ok(d) if k == 0 => d.0,
        Ok(_) if k == 1 => 11,
        Err((a, _)) if k == 2 => a.0,
        Err((_, b)) => b.0 + k,
        Ok(d) => d.0 + 100,
    }
}
#[test] fn drop_order() {
    take();
    assert_eq!(drops(Ok(D(1)), 0), 1); assert_eq!(take(), vec![901]);
    assert_eq!(drops(Ok(D(1)), 1), 11); assert_eq!(take(), vec![901]);
    assert_eq!(drops(Ok(D(1)), 5), 101); assert_eq!(take(), vec![901]);
    assert_eq!(drops(Err((D(2), D(3))), 2), 2); assert_eq!(take(), vec![902, 903]);
    assert_eq!(drops(Err((D(2), D(3))), 7), 10); assert_eq!(take(), vec![903, 902]);
}

// bool and nested matches on a state machine (the tokenizer shape)
#[derive(Clone, Copy, PartialEq, Debug)]
enum St { Start, Ident, Num, Str, Esc, Slash, Line }
fn step(st: St, c: u8) -> (St, bool) {
    match st {
        St::Start => match c {
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => (St::Ident, false),
            b'0'..=b'9' => (St::Num, false),
            b'"' => (St::Str, false),
            b'/' => (St::Slash, false),
            b' ' | b'\n' => (St::Start, false),
            _ => (St::Start, true),
        },
        St::Ident => match c { b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'0'..=b'9' => (St::Ident, false), _ => (St::Start, true) },
        St::Num => match c { b'0'..=b'9' | b'.' => (St::Num, false), _ => (St::Start, true) },
        St::Str => match c { b'\\' => (St::Esc, false), b'"' => (St::Start, true), _ => (St::Str, false) },
        St::Esc => (St::Str, false),
        St::Slash => match c { b'/' => (St::Line, false), _ => (St::Start, true) },
        St::Line => match c { b'\n' => (St::Start, true), _ => (St::Line, false) },
    }
}
#[test] fn state_machine() {
    let src = b"let x_1 = 42.5 + y; // c\n\"s\\\"q\" /";
    let mut st = St::Start;
    let (mut h, mut n) = (0u64, 0u32);
    for &c in src.iter() {
        let (s2, emit) = step(st, c);
        h = h.wrapping_mul(31).wrapping_add(s2 as u64 + 1);
        if emit { n += 1; }
        st = s2;
    }
    assert_eq!((n, st), (8, St::Slash));
    let _ = h;
}

fn flags(a: bool, b: bool, c: Option<bool>) -> u32 {
    match (a, b, c) {
        (true, true, Some(true)) => 1,
        (true, _, None) => 2,
        (false, true, _) => 3,
        (_, false, Some(false)) => 4,
        (true, true, Some(false)) => 5,
        (false, false, _) => 6,
        (true, false, _) => 7,
    }
}
#[test] fn booleans() {
    let mut seen = [0u32; 8];
    for a in [false, true] { for b in [false, true] { for c in [None, Some(false), Some(true)] {
        seen[flags(a, b, c) as usize] += 1;
    }}}
    assert_eq!(seen, [0, 1, 2, 3, 2, 1, 2, 1]);
}

// consts, strings and slices in a match with more tree-shaped arms
const K: u32 = 77;
fn mixed(x: u32, s: &str, v: &[u8]) -> u32 {
    match (x, s) {
        (K, _) => 1,
        (0, "a") => 2,
        (1..=5, "b" | "c") => 3,
        (_, "zz") => 4,
        (_, _) if v.len() == 3 => 5,
        (9, _) => match v { [] => 6, [1, ..] => 7, [.., 9] => 8, _ => 9 },
        _ => 10,
    }
}
#[test] fn consts_strings_slices() {
    assert_eq!(mixed(77, "q", &[]), 1);
    assert_eq!(mixed(0, "a", &[]), 2);
    assert_eq!(mixed(3, "c", &[]), 3);
    assert_eq!(mixed(3, "zz", &[]), 4);
    assert_eq!(mixed(3, "q", &[1, 2, 3]), 5);
    assert_eq!(mixed(9, "q", &[]), 6);
    assert_eq!(mixed(9, "q", &[1, 5]), 7);
    assert_eq!(mixed(9, "q", &[5, 9]), 8);
    assert_eq!(mixed(9, "q", &[5, 8]), 9);
    assert_eq!(mixed(8, "q", &[5, 8]), 10);
}

// constants the back ends cannot encode as 32-bit immediates: ids (hashes with the top bit set),
// values around the 32-bit limits, negative ones
#[derive(Clone, Copy, PartialEq, Debug)]
struct Id(u64);
fn ids(id: Id) -> u32 {
    match id {
        Id(0x8d3c_7a11_9f00_0001) => 1,
        Id(0x0000_133f_1995_ee71) => 2,
        Id(0xffff_ffff) => 3,
        Id(0x1_0000_0000..=0x1_0000_00ff) => 4,
        Id(0x7fff_ffff) | Id(0x8000_0000) => 5,
        Id(u64::MAX) => 6,
        Id(0xffff_ffff_0000_0000..=0xffff_ffff_0000_000f) => 7,
        _ => 8,
    }
}
#[test] fn wide_constants() {
    for (v, w) in [
        (0x8d3c_7a11_9f00_0001u64, 1), (0x0000_133f_1995_ee71, 2), (0xffff_ffff, 3), (0x1_0000_0000, 4), (0x1_0000_00ff, 4), (0x1_0000_0100, 8),
        (0x7fff_ffff, 5), (0x8000_0000, 5), (0x8000_0001, 8), (u64::MAX, 6), (u64::MAX - 1, 8), (0xffff_ffff_0000_0000, 7), (0xffff_ffff_0000_000f, 7),
        (0xffff_ffff_0000_0010, 8), (0, 8), (0xfffe_ffff, 8),
    ] {
        assert_eq!(ids(Id(v)), w, "{v:#x}");
    }
}
fn signed_wide(x: i64) -> u32 {
    match x {
        i64::MIN => 1,
        -0x8000_0001 => 2,
        -0x8000_0000..=-1 => 3,
        0 => 4,
        1..=0x7fff_ffff => 5,
        0x8000_0000 | 0x1_0000_0000 => 6,
        0x8000_0001..=i64::MAX => 7,
        _ => 8,
    }
}
#[test] fn wide_signed_constants() {
    for (v, w) in [
        (i64::MIN, 1), (-0x8000_0001, 2), (-0x8000_0000, 3), (-1, 3), (0, 4), (1, 5), (0x7fff_ffff, 5), (0x8000_0000, 6), (0x1_0000_0000, 6),
        (0x8000_0001, 7), (i64::MAX, 7), (i64::MIN + 1, 8), (-0x8000_0002, 8),
    ] {
        assert_eq!(signed_wide(v), w, "{v:#x}");
    }
}
