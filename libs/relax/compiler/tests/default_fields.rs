// Default field values (rustc's default_field_values syntax): `S { c: 1, .. }` takes the omitted
// fields from their declared defaults -- anonymous consts, evaluated as consts. `.rx` structs give
// every field a default.
const N: usize = 3;

struct S {
    a: u32 = 7,
    b: String = String::new(),
    c: u8,
}

enum E {
    V { x: i32 = -1, y: i32 },
    W,
}

struct A {
    v: [u8; N] = [1; N],
    n: usize = N * 2,
    neg: i64 = -(N as i64),
}

#[derive(Debug, PartialEq)]
struct P {
    x: f32 = 0.5,
    y: f32 = 1.5,
}

#[test]
fn default_fields() {
    let s = S { c: 1, .. };
    assert_eq!((s.a, s.b.len(), s.c), (7, 0, 1));
    let s2 = S { a: 2, c: 3, .. };
    assert_eq!((s2.a, s2.c), (2, 3));
    let e = E::V { y: 5, .. };
    match e {
        E::V { x, y } => assert_eq!((x, y), (-1, 5)),
        E::W => panic!(),
    }
    let a = A { .. };
    assert_eq!((a.v, a.n, a.neg), ([1, 1, 1], 6, -3));
    let p = P { y: 2.0, .. };
    assert_eq!(p, P { x: 0.5, y: 2.0 });
    let all = P { .. };
    assert_eq!(all, P { x: 0.5, y: 1.5 });
    let _ = E::W;
    println!("| defaults ok");
}
