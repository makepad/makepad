// A match arm with several or-alternatives and an `if let` guard that binds: each alternative is a
// candidate of its own, and the guard's bindings must have their storage on every candidate's path,
// not only on the first one lowered (the later alternatives read the bindings' slots too). In a
// baseline body (RELAX_O1=always) a later alternative bound into an address its path never set.
// Lines starting with "| " must equal real std's (agree.sh, also under RELAX_O1=always).

#[derive(Clone, Copy)]
enum Ev {
    Press,
    Move,
    Release,
    Cancel,
}

struct Point(f32, f32);

#[inline(never)]
fn sum(p: &Point) -> f32 {
    p.0 + p.1
}

#[inline(never)]
fn two_alternatives(e: Ev, p: Option<Point>) -> f32 {
    match e {
        Ev::Press => -1.0,
        Ev::Move | Ev::Release if let Some(q) = p => sum(&q),
        _ => 0.0,
    }
}

#[inline(never)]
fn three_alternatives_owned(e: Ev, name: Option<String>) -> String {
    match e {
        Ev::Move | Ev::Release | Ev::Cancel if let Some(n) = name => n + "!",
        _ => String::from("none"),
    }
}

#[inline(never)]
fn nested_alternatives(e: Ev, k: u8, v: Option<(u32, String)>) -> u32 {
    match (e, k) {
        (Ev::Press | Ev::Cancel, 1 | 2) if let Some((x, s)) = &v => *x + s.len() as u32 + k as u32,
        (_, 3) => 300,
        _ => 7,
    }
}

#[inline(never)]
fn guard_fails_then_next_arm(e: Ev, p: Option<Point>) -> f32 {
    match e {
        Ev::Move | Ev::Release if let Some(q) = &p && q.0 > 0.0 => sum(q),
        Ev::Move | Ev::Release => 10.0,
        _ => 20.0,
    }
}

#[inline(never)]
fn len_of(s: &String) -> usize {
    s.len()
}

#[inline(never)]
fn block_let_in_guard(e: Ev, n: u32) -> u32 {
    match e {
        Ev::Move | Ev::Release if { let s = n.to_string(); len_of(&s) > 1 } => 1,
        _ => 0,
    }
}

#[inline(never)]
fn match_by_ref_in_guard(e: Ev, v: Option<String>) -> usize {
    match e {
        Ev::Move | Ev::Release if match &v { Some(s) => len_of(s) > 2, None => false } => 1,
        Ev::Cancel | Ev::Press if if let Some(ref s) = v { len_of(s) == 1 } else { false } => 2,
        _ => 0,
    }
}

// two sibling blocks, each with an address-taken `let`, and locals after them: every one gets its
// storage at the arm's entry, and none may share it with another the guard still uses
#[inline(never)]
fn sibling_blocks_in_guard(e: Ev, n: u32) -> u32 {
    match e {
        Ev::Move | Ev::Release if {
            let a = { let s = n.to_string(); let mut x = s.clone(); x.push('x'); x };
            let b = { let t = (n * 7).to_string(); len_of(&t) };
            let c = len_of(&a) * 10 + b;
            let r = &c;
            println!("| sibling blocks: {a} {b} {}", *r);
            *r > 30
        } => n,
        _ => 0,
    }
}

#[inline(never)]
fn closure_in_guard(e: Ev, n: u32) -> u32 {
    let k = n + 1;
    match e {
        Ev::Move | Ev::Release if (|| { let t = k * 2; len_of(&t.to_string()) > 1 })() => k,
        _ => 0,
    }
}

#[test]
fn a01_every_alternative_reads_the_guard_binding() {
    let r = [
        two_alternatives(Ev::Release, Some(Point(2.0, 3.0))),
        two_alternatives(Ev::Move, Some(Point(3.0, 4.0))),
        two_alternatives(Ev::Release, None),
        two_alternatives(Ev::Move, None),
        two_alternatives(Ev::Press, Some(Point(2.0, 3.0))),
        two_alternatives(Ev::Cancel, Some(Point(2.0, 3.0))),
    ];
    println!("| two alternatives: {r:?}");
    assert_eq!(r, [5.0, 7.0, 0.0, 0.0, -1.0, 0.0]);
}

#[test]
fn a02_an_owned_binding_on_the_last_alternative() {
    let r = [
        three_alternatives_owned(Ev::Move, Some("a".into())),
        three_alternatives_owned(Ev::Release, Some("b".into())),
        three_alternatives_owned(Ev::Cancel, Some("c".into())),
        three_alternatives_owned(Ev::Cancel, None),
        three_alternatives_owned(Ev::Press, Some("d".into())),
    ];
    println!("| three alternatives, owned: {r:?}");
    assert_eq!(r, ["a!", "b!", "c!", "none", "none"]);
}

#[test]
fn a03_nested_or_patterns_give_four_candidates() {
    let v = || Some((40u32, String::from("xy")));
    let r = [
        nested_alternatives(Ev::Press, 1, v()),
        nested_alternatives(Ev::Press, 2, v()),
        nested_alternatives(Ev::Cancel, 1, v()),
        nested_alternatives(Ev::Cancel, 2, v()),
        nested_alternatives(Ev::Cancel, 2, None),
        nested_alternatives(Ev::Cancel, 3, v()),
        nested_alternatives(Ev::Move, 1, v()),
    ];
    println!("| nested alternatives: {r:?}");
    assert_eq!(r, [43, 44, 43, 44, 7, 300, 7]);
}

#[test]
fn a04_a_failing_guard_falls_through_on_every_alternative() {
    let r = [
        guard_fails_then_next_arm(Ev::Move, Some(Point(1.0, 2.0))),
        guard_fails_then_next_arm(Ev::Release, Some(Point(1.5, 2.0))),
        guard_fails_then_next_arm(Ev::Release, Some(Point(-1.0, 2.0))),
        guard_fails_then_next_arm(Ev::Release, None),
        guard_fails_then_next_arm(Ev::Press, None),
    ];
    println!("| guard fails, next arm: {r:?}");
    assert_eq!(r, [3.0, 3.5, 10.0, 10.0, 20.0]);
}

#[test]
fn a05_nested_bindings_in_the_guard() {
    let r = [
        block_let_in_guard(Ev::Move, 5),
        block_let_in_guard(Ev::Release, 50),
        block_let_in_guard(Ev::Release, 5),
        block_let_in_guard(Ev::Press, 50),
    ];
    println!("| block let in guard: {r:?}");
    assert_eq!(r, [0, 1, 0, 0]);
    let r = [
        match_by_ref_in_guard(Ev::Move, Some("abc".into())),
        match_by_ref_in_guard(Ev::Release, Some("abcd".into())),
        match_by_ref_in_guard(Ev::Release, Some("a".into())),
        match_by_ref_in_guard(Ev::Release, None),
        match_by_ref_in_guard(Ev::Cancel, Some("a".into())),
        match_by_ref_in_guard(Ev::Press, Some("b".into())),
        match_by_ref_in_guard(Ev::Press, Some("bb".into())),
    ];
    println!("| match and if let by reference in guard: {r:?}");
    assert_eq!(r, [1, 1, 0, 0, 2, 2, 0]);
    let r = [
        closure_in_guard(Ev::Move, 3),
        closure_in_guard(Ev::Release, 30),
        closure_in_guard(Ev::Release, 3),
        closure_in_guard(Ev::Cancel, 30),
    ];
    println!("| closure in guard: {r:?}");
    assert_eq!(r, [0, 31, 0, 0]);
}

#[test]
fn a06_sibling_blocks_in_the_guard() {
    let r = [
        sibling_blocks_in_guard(Ev::Move, 5),
        sibling_blocks_in_guard(Ev::Release, 50),
        sibling_blocks_in_guard(Ev::Release, 2),
        sibling_blocks_in_guard(Ev::Move, 20),
        sibling_blocks_in_guard(Ev::Press, 50),
    ];
    println!("| sibling blocks in guard: {r:?}");
    assert_eq!(r, [0, 50, 0, 20, 0]);
}
