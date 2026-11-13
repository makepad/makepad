// The by-reference and plain-integer operator impls of num::Wrapping<T>: `a op &b`, `&a op b`,
// `&a op &b` (stable since 1.14), `a op= &b` (1.22), `a op= int` and `a op= &int` (1.60), the
// shifts by `&usize` (1.39), `-&a`, `!&a`, unary minus on the unsigned types, and `/=`, `%=`.
// ring 0.17's SHA-1 adds a `&Wrapping<u32>` it gets from a slice iterator (src/digest/sha1.rs:
// `let T = rotl(a, 5) + f(b, c, d) + e + k + W_t;`).
use std::num::Wrapping;

type W32 = Wrapping<u32>;

// ring's shape
fn rotl(x: W32, n: u32) -> W32 {
    Wrapping(x.0.rotate_left(n))
}
fn step3(mut a: W32, mut b: W32, mut c: W32, mut d: W32, mut e: W32, w: &[W32; 8], k: W32, f: impl Fn(W32, W32, W32) -> W32) -> (W32, W32, W32, W32, W32) {
    let w = &w[2..6];
    for w_t in w.iter() {
        let t = rotl(a, 5) + f(b, c, d) + e + k + w_t;
        e = d;
        d = c;
        c = rotl(b, 30);
        b = a;
        a = t;
    }
    (a, b, c, d, e)
}
fn step3_plain(mut a: u32, mut b: u32, mut c: u32, mut d: u32, mut e: u32, w: &[u32], k: u32) -> (u32, u32, u32, u32, u32) {
    for w_t in w {
        let t = a.rotate_left(5).wrapping_add(b ^ c ^ d).wrapping_add(e).wrapping_add(k).wrapping_add(*w_t);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    (a, b, c, d, e)
}

#[test]
fn ring_sha1_round_adds_a_borrowed_word() {
    let w: [W32; 8] = [1, 0xffff_fff0, 0x8000_0001, 0xdead_beef, 0x0bad_f00d, 0xffff_ffff, 7, 9].map(Wrapping);
    let got = step3(Wrapping(0x67452301), Wrapping(0xefcdab89), Wrapping(0x98badcfe), Wrapping(0x10325476), Wrapping(0xc3d2e1f0), &w, Wrapping(0x6ed9eba1), |x, y, z| x ^ y ^ z);
    let want = step3_plain(0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0, &[0x8000_0001, 0xdead_beef, 0x0bad_f00d, 0xffff_ffff], 0x6ed9eba1);
    assert_eq!((got.0 .0, got.1 .0, got.2 .0, got.3 .0, got.4 .0), want);
    assert_eq!(got.0 .0, 0x5a473d66);
    let total: W32 = w.iter().fold(Wrapping(0), |acc, x| acc + x);
    assert_eq!(total.0, 0x6a5baefd);
}

// every operand form of a binary operator and of its `op=` twin agrees with the integer's
// wrapping method
macro_rules! binop_forms {
    ($t:ty, $a:expr, $b:expr, $($op:tt $op_assign:tt $method:ident $tra:ident $ma:ident)*) => {{
        let (a, b): (Wrapping<$t>, Wrapping<$t>) = (Wrapping($a), Wrapping($b));
        $(
            let want: Wrapping<$t> = Wrapping(a.0.$method(b.0));
            assert_eq!(a $op b, want, "{} {} {} {}", stringify!($t), a, stringify!($op), b);
            assert_eq!(a $op &b, want);
            assert_eq!(&a $op b, want);
            assert_eq!(&a $op &b, want);
            let mut x = a;
            x $op_assign b;
            assert_eq!(x, want);
            let mut x = a;
            x $op_assign &b;
            assert_eq!(x, want);
            // `x op= int` by value: through the trait method as well, and for the 128-bit words
            // only so (the JIT miscompiles operator syntax with a by-value u128/i128 right-hand
            // side on a struct, whatever the impl)
            let mut x = a;
            $tra::<$t>::$ma(&mut x, b.0);
            assert_eq!(x, want);
            if std::mem::size_of::<$t>() <= 8 {
                let mut x = a;
                x $op_assign b.0;
                assert_eq!(x, want, "{} {} {} {}", stringify!($t), a, stringify!($op_assign), b);
            }
            let mut x = a;
            x $op_assign &b.0;
            assert_eq!(x, want, "{} {} {} &{}", stringify!($t), a, stringify!($op_assign), b);
        )*
    }};
}
macro_rules! all_binops {
    ($($t:ty: $a:expr, $b:expr;)*) => {$(
        binop_forms!($t, $a, $b,
            + += wrapping_add AddAssign add_assign
            - -= wrapping_sub SubAssign sub_assign
            * *= wrapping_mul MulAssign mul_assign
            / /= wrapping_div DivAssign div_assign
            % %= wrapping_rem RemAssign rem_assign
            ^ ^= bitxor BitXorAssign bitxor_assign
            | |= bitor BitOrAssign bitor_assign
            & &= bitand BitAndAssign bitand_assign);
    )*};
}

#[test]
fn binary_operators_take_references_and_plain_integers() {
    use std::ops::{AddAssign, BitAnd, BitAndAssign, BitOr, BitOrAssign, BitXor, BitXorAssign, DivAssign, MulAssign, RemAssign, SubAssign};
    all_binops! {
        u8: 200, 77;
        u16: 0xfff0, 0x0123;
        u32: 0xdead_beef, 0x9000_0003;
        u64: u64::MAX - 5, 0x1_0000_0007;
        u128: u128::MAX / 3, (1 << 100) + 9;
        usize: usize::MAX - 1, 12345;
        i8: -128, -1;
        i16: i16::MIN, -1;
        i32: i32::MIN, -1;
        i64: i64::MIN, -1;
        i128: i128::MIN, -1;
        isize: isize::MIN, -1;
        i32: 0x7fff_fff0, 0x1234;
        i64: -0x7fff_ffff_ffff_0000, 0x0fed_cba9;
    }
}

macro_rules! shift_and_unary_forms {
    ($($t:ty: $a:expr;)*) => {$(
        let a: Wrapping<$t> = Wrapping($a);
        // a shift count past the width wraps around it
        for n in [0usize, 1, 5, 7, 9, 70, 200] {
            let (l, r): (Wrapping<$t>, Wrapping<$t>) = (Wrapping(a.0.wrapping_shl(n as u32)), Wrapping(a.0.wrapping_shr(n as u32)));
            assert_eq!(a << n, l, "{} {} << {}", stringify!($t), a, n);
            assert_eq!(a << &n, l);
            assert_eq!(&a << n, l);
            assert_eq!(&a << &n, l);
            assert_eq!(a >> n, r, "{} {} >> {}", stringify!($t), a, n);
            assert_eq!(a >> &n, r);
            assert_eq!(&a >> n, r);
            assert_eq!(&a >> &n, r);
            let mut x = a;
            x <<= n;
            assert_eq!(x, l);
            let mut x = a;
            x <<= &n;
            assert_eq!(x, l);
            let mut x = a;
            x >>= n;
            assert_eq!(x, r);
            let mut x = a;
            x >>= &n;
            assert_eq!(x, r);
        }
        assert_eq!(-a, Wrapping(a.0.wrapping_neg()), "{} -{}", stringify!($t), a);
        assert_eq!(-&a, Wrapping(a.0.wrapping_neg()));
        assert_eq!(!a, Wrapping(!a.0));
        assert_eq!(!&a, Wrapping(!a.0));
    )*};
}

#[test]
fn shifts_and_unary_operators_take_references() {
    shift_and_unary_forms! {
        u8: 0x81;
        u16: 0x8001;
        u32: 0x8000_0001;
        u64: 0x8000_0000_0000_0001;
        u128: (1 << 127) + 1;
        usize: usize::MAX - 2;
        i8: i8::MIN;
        i16: -2;
        i32: i32::MIN;
        i64: 0x4000_0000_0000_0001;
        i128: i128::MIN;
        isize: -3;
    }
    // unary minus on an unsigned word is 0 - x
    assert_eq!(-Wrapping(1u32), Wrapping(u32::MAX));
    assert_eq!(-Wrapping(0u64), Wrapping(0));
    assert_eq!((-Wrapping(3u8)).0, 253);
    // generic code over the reference forms
    fn sum_refs<'a, T: 'a + Copy>(zero: T, xs: &'a [T]) -> T
    where
        T: std::ops::Add<&'a T, Output = T>,
    {
        xs.iter().fold(zero, |acc, x| acc + x)
    }
    assert_eq!(sum_refs(Wrapping(250u8), &[Wrapping(3), Wrapping(4)]), Wrapping(1));
}
