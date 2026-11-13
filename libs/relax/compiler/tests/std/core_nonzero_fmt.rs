// The formatting traits of NonZero<T> beyond Debug and Display: Binary, Octal, LowerHex, UpperHex
// (stable since 1.28), LowerExp and UpperExp (1.84), each the inner integer's own impl. rustix
// 1.1's `Pid(NonZeroI32)` (src/pid.rs) implements all of them with `self.0.fmt(f)`, which failed
// with "no method named `fmt` found for `NonZero<i32>`".
//
// The same file holds the two sibling shapes over a plain integer, which need the integers'
// LowerExp and UpperExp: rustix's `Uid(u32)` (src/ugid.rs) and deranged 0.5's ranged integers
// (src/lib.rs, `self.get().fmt(f)` in every fmt trait). Inside `impl fmt::Binary for X` the
// call `self.0.fmt(f)` finds only the trait being implemented: no fmt trait is imported.
use core::fmt;
use core::num::{NonZero, NonZeroI32, NonZeroU128, NonZeroU8};

macro_rules! forward_fmt {
    ($ty:ty => $($tr:ident)+) => {
        $(impl fmt::$tr for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        })+
    };
}

// rustix's Pid and Uid
struct Pid(NonZeroI32);
struct Uid(u32);
forward_fmt!(Pid => Display Binary Octal LowerHex UpperHex LowerExp UpperExp);
forward_fmt!(Uid => Display Binary Octal LowerHex UpperHex LowerExp UpperExp);

// deranged's shape: a const-generic wrapper whose impls go through a getter
struct RangedI16<const MIN: i16, const MAX: i16>(i16);
impl<const MIN: i16, const MAX: i16> RangedI16<MIN, MAX> {
    const fn get(self) -> i16 {
        self.0
    }
}
impl<const MIN: i16, const MAX: i16> Clone for RangedI16<MIN, MAX> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<const MIN: i16, const MAX: i16> Copy for RangedI16<MIN, MAX> {}
macro_rules! ranged_fmt {
    ($($tr:ident)+) => {
        $(impl<const MIN: i16, const MAX: i16> fmt::$tr for RangedI16<MIN, MAX> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.get().fmt(f)
            }
        })+
    };
}
ranged_fmt!(Debug Display Binary Octal LowerHex UpperHex LowerExp UpperExp);

/// Every radix and exponent form of `$a`, with flags, as one string.
macro_rules! forms {
    ($a:expr) => {
        format!(
            "{:b} {:o} {:x} {:X} {:e} {:E} | {:#b} {:#o} {:#x} {:#X} | {:#010x} {:>12b} {:<8o}| {:+e} {:.2e} {:12.3E} {:^9X}|",
            $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a, $a
        )
    };
}

#[test]
fn a01_nonzero_formats_as_its_integer() {
    let n = NonZeroI32::new(1234).unwrap();
    assert_eq!(format!("{:b}", n), "10011010010");
    assert_eq!(format!("{:o}", n), "2322");
    assert_eq!(format!("{:x}", n), "4d2");
    assert_eq!(format!("{:X}", n), "4D2");
    assert_eq!(format!("{:e}", n), "1.234e3");
    assert_eq!(format!("{:E}", n), "1.234E3");
    assert_eq!(forms!(n), forms!(1234i32));

    // negative values print their two's complement in a radix, their sign with an exponent
    let m = NonZeroI32::new(-1200).unwrap();
    assert_eq!(format!("{:x} {:#X} {:e} {:.0E}", m, m, m, m), "fffffb50 0xFFFFFB50 -1.2e3 -1E3");
    assert_eq!(forms!(m), forms!(-1200i32));

    macro_rules! each {
        ($($int:ident: $($v:expr),+;)+) => {
            $($(
                let nz = NonZero::<$int>::new($v).unwrap();
                assert_eq!(forms!(nz), forms!($v as $int), "{} {}", stringify!($int), $v);
                assert_eq!(forms!(&nz), forms!($v as $int));
            )+)+
        };
    }
    each! {
        i8: 1, -1, i8::MIN, i8::MAX;
        i16: 300, -300, i16::MIN;
        i32: 99_950, i32::MIN, i32::MAX;
        i64: -1, i64::MAX, i64::MIN;
        i128: 7, i128::MIN + 1, i128::MAX;
        isize: -65_536, isize::MAX;
        u8: 1, 200, u8::MAX;
        u16: 4096, u16::MAX;
        u32: 0xdead_beef_u32, 10;
        u64: u64::MAX, 1u64 << 40;
        u128: u128::MAX, 1;
        usize: usize::MAX, 255;
    }
    assert_eq!(format!("{:x}", NonZeroU8::MAX), "ff");
    assert_eq!(format!("{:e}", NonZeroU128::MAX), "3.40282366920938463463374607431768211455e38");
}

#[test]
fn a02_wrappers_forward_through_the_trait_being_implemented() {
    let pid = Pid(NonZeroI32::new(4321).unwrap());
    assert_eq!(format!("{} {:b} {:o} {:x} {:X} {:e} {:E}", pid, pid, pid, pid, pid, pid, pid), "4321 1000011100001 10341 10e1 10E1 4.321e3 4.321E3");
    assert_eq!(forms!(pid), forms!(4321i32));

    let uid = Uid(501);
    assert_eq!(format!("{} {:b} {:o} {:x} {:X} {:e} {:E}", uid, uid, uid, uid, uid, uid, uid), "501 111110101 765 1f5 1F5 5.01e2 5.01E2");
    assert_eq!(forms!(uid), forms!(501u32));
    let root = Uid(0);
    assert_eq!(format!("{:e} {:#x} {:.1E}", root, root, root), "0e0 0x0 0.0E0");

    let r = RangedI16::<-999, 999>(-250);
    assert_eq!(format!("{:?} {} {:b} {:o} {:x} {:X} {:e} {:E}", r, r, r, r, r, r, r, r), "-250 -250 1111111100000110 177406 ff06 FF06 -2.5e2 -2.5E2");
    assert_eq!(forms!(r), forms!(-250i16));
}
