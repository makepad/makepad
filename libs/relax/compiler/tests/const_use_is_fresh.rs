// Every use of a const is a fresh value (rustc: a `Constant` operand copied out of the
// immutable evaluated allocation; only promoted shared borrows point into static memory).
// Relax bound `let mut h = C;` to the constant's own storage, so `h[0] += 100` changed C for
// every later use (sha256's H0: wrong cache keys and archive checksums).
const C: [u32; 4] = [1, 2, 3, 4];
const S: Pair = Pair { a: [7; 3], b: 9 };
#[derive(Clone, Copy, Debug, PartialEq)]
struct Pair { a: [u64; 3], b: u64 }
struct Holder { arr: [u32; 4], p: Pair }

fn bump(h: &mut [u32; 4]) { h[0] = h[0].wrapping_add(100); }
fn by_value(mut h: [u32; 4]) -> u32 { h[1] += 50; h[1] }

#[test]
fn let_mut_binding() {
    let mut h = C;
    h[0] += 100;
    assert_eq!(h, [101, 2, 3, 4]);
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn write_through_mut_ref() {
    let mut g = C;
    bump(&mut g);
    assert_eq!(g[0], 101);
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn struct_field_from_const() {
    let mut s = S;
    s.a[0] = 1;
    s.b += 1;
    let mut h = Holder { arr: C, p: S };
    h.arr[3] = 0;
    h.p.a[2] = 0;
    assert_eq!(s, Pair { a: [1, 7, 7], b: 10 });
    assert_eq!(S, Pair { a: [7; 3], b: 9 });
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn array_of_consts() {
    let mut a = [C, C];
    a[0][0] = 9;
    a[1][1] = 9;
    assert_eq!(a, [[9, 2, 3, 4], [1, 9, 3, 4]]);
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn fn_arg_by_value() {
    assert_eq!(by_value(C), 52);
    assert_eq!(by_value(C), 52);
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn match_scrutinee() {
    let r = match C {
        mut x => { x[2] = 0; x }
    };
    assert_eq!(r, [1, 2, 0, 4]);
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
#[allow(const_item_mutation)]
fn mut_ref_to_const() {
    // `&mut C` borrows a fresh temporary (rustc lint const_item_mutation)
    let r = &mut C;
    r[0] = 77;
    assert_eq!(C, [1, 2, 3, 4]);
    C[1] = 5;
    assert_eq!(C, [1, 2, 3, 4]);
}

#[test]
fn shared_promotion_unchanged() {
    let r: &'static [u32; 4] = &C;
    let mut h = C;
    h[3] = 0;
    assert_eq!(*r, [1, 2, 3, 4]);
}

#[test]
fn inline_const_and_assoc_const() {
    struct K;
    impl K { const A: [u8; 3] = [1, 2, 3]; }
    let mut x = const { [5u16; 4] };
    x[0] = 0;
    assert_eq!(const { [5u16; 4] }, [5; 4]);
    let mut y = K::A;
    y[0] = 9;
    assert_eq!(K::A, [1, 2, 3]);
}
