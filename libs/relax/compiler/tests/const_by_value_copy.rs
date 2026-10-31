// A `const` used by value is a fresh copy at each use (Reference: constant items are inlined
// wherever they are used); only `&CONST` may share promoted storage, and that cannot be written.
// A local initialised from a const and then mutated must never write through to the const:
// sha256 copies its IV into its state and mutates it, and every hash after the first in a
// process came out wrong. rustc prints the same lines.
use std::hint::black_box;

const IV: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    a: u64,
    b: [u16; 4],
    c: u8,
}

const START: State = State { a: 7, b: [1, 2, 3, 4], c: 9 };

struct Hasher;
impl Hasher {
    const INIT: [u64; 4] = [11, 22, 33, 44];
}

// the sha256 pattern: copy the const into the state, mutate it in a loop, return it
fn rounds(n: u32) -> [u32; 8] {
    let mut s = IV;
    for i in 0..n {
        let t = s[7].wrapping_add(i).rotate_left(5) ^ s[0];
        for j in (1..8).rev() {
            s[j] = s[j - 1].wrapping_add(t);
        }
        s[0] = t;
    }
    s
}

fn struct_copy() -> State {
    let mut s = START;
    s.a += 1;
    s.b[2] = 100;
    s.c = s.c.wrapping_mul(3);
    s
}

fn assoc_copy() -> [u64; 4] {
    let mut h = Hasher::INIT;
    h[0] = h[0].wrapping_add(1);
    h[3] *= 2;
    h
}

fn bump(h: &mut [u32; 8]) {
    for x in h.iter_mut() {
        *x = x.wrapping_add(1);
    }
}

mod inner {
    // a const reached through a module path, written through `&mut` in a callee
    pub fn through_mut() -> [u32; 8] {
        let mut h = super::IV;
        super::bump(&mut h);
        h
    }
}

fn borrowed() -> &'static [u32; 8] {
    &IV
}

#[test]
fn const_by_value_copy() {
    // (a) array const copied into a mut local and mutated, then the const read again
    let mut h = IV;
    h[0] = h[0].wrapping_add(1);
    black_box(&mut h);
    assert_eq!(h[0], 0x6a09e668);
    assert_eq!(IV[0], 0x6a09e667);
    let again = IV;
    assert_eq!(again[0], 0x6a09e667);

    // (b) a struct const the same way
    let s1 = struct_copy();
    let s2 = struct_copy();
    assert_eq!(s1, s2);
    assert_eq!(s1, State { a: 8, b: [1, 2, 100, 4], c: 27 });
    assert_eq!(START, State { a: 7, b: [1, 2, 3, 4], c: 9 });

    // (c) a mutated copy in a loop, called twice: the same result both times
    let r1 = rounds(black_box(64));
    let r2 = rounds(black_box(64));
    assert_eq!(r1, r2);
    assert_eq!(IV[7], 0x5be0cd19);

    // (d) an associated const
    let a1 = assoc_copy();
    let a2 = assoc_copy();
    assert_eq!(a1, a2);
    assert_eq!(a1, [12, 22, 33, 88]);
    assert_eq!(Hasher::INIT, [11, 22, 33, 44]);

    // (c') the IV written through `&mut` in a callee, reached through a module path
    let m1 = inner::through_mut();
    let m2 = inner::through_mut();
    assert_eq!(m1, m2);
    assert_eq!(m1[0], 0x6a09e668);
    assert_eq!(IV[0], 0x6a09e667);

    // `&mut CONST` borrows a fresh temporary
    #[allow(const_item_mutation)]
    {
        let t = &mut IV;
        t[0] = 5;
        bump(&mut IV);
        assert_eq!(t[0], 5);
    }
    assert_eq!(IV[0], 0x6a09e667);

    // (e) `&CONST` borrows still work (and may share storage: they cannot be written)
    let b = borrowed();
    assert_eq!(b, &IV);
    assert_eq!(b[0], 0x6a09e667);
    let r = &IV;
    assert_eq!(r[3], 0xa54ff53a);

    println!("{:08x} {:08x} {:?} {:?} {:?}", r1[0], r1[7], s1, a1, b[0]);
}
