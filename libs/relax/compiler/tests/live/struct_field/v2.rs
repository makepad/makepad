// G3 / 4.4: struct layout edits (no live values of the type survive between runs).
//  v1: a field is added between two others; make() and sum() are edited to use it. Every fn that
//      uses P's layout (offsets are baked into code) must be recompiled, including outer(), whose
//      text does not change but which embeds P in Outer.
//  v2: only the struct's field order changes (no fn text changes): the derived Debug prints the
//      new order, and every user of the layout is recompiled.
#[derive(Debug, Clone, Copy)]
struct P {
    b: u64,
    c: u16,
    a: u32,
}

struct Outer {
    tag: u8,
    ps: [P; 3],
}

fn make(i: u32) -> P {
    P { a: i, c: (i * 3) as u16, b: (i as u64) << 33 }
}

fn sum(p: &P) -> u64 {
    p.a as u64 + p.b + p.c as u64 * 1000
}

fn outer() -> u64 {
    let o = Outer { tag: 7, ps: [make(1), make(2), make(3)] };
    let mut s = o.tag as u64;
    for p in o.ps.iter() {
        s = s.wrapping_mul(31).wrapping_add(sum(p));
    }
    s
}

#[test]
fn run() {
    println!("out {:?} outer {}", make(5), outer());
}
