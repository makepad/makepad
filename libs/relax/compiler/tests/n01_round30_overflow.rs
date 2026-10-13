// N:rustc_mir_build/src/builder/mod.rs:777-785; expr/as_rvalue.rs:103-123,457-589.
static STATIC: u8 = { let x = 254u8; x + 1 };
struct Owner;
impl Owner { const VALUE: u8 = { let x = 254u8; x + 1 }; }
const ANON: [u8; { let x = 1usize; x + 1 }] = [3, 3];
const U8: u8 = { let x = 254u8; x + 1 };
const I8: i8 = { let x = -127i8; x - 1 };
const NEG_LITERAL: i8 = -128i8;
const U64: u64 = { let x = 0xffff_ffffu64; x * x };
const I64: i64 = { let x = -3i64; x * 7 };
const U128: u128 = { let x = 1u128 << 100; x * 3 };
const I128: i128 = { let x = -(1i128 << 100); x * 3 };
const I128_MIN: i128 = -170141183460469231731687303715884105728i128;
const I128_SIGN: i128 = { let x = -1i128; x * I128_MIN.wrapping_add(1) };
const SHIFT: u8 = { let x = 7u128; 1u8 << x };
const MASKED: u8 = { let x = 255u8; x.wrapping_add(1) };
const OVERFLOWING: (u8, bool) = { let x = 255u8; x.overflowing_add(1) };
const BOOL: bool = !false;
const CAST: usize = (300u32 as u8) as usize;
const fn release_add(x: u8) -> u8 { x + 1 }
const RELEASE: u8 = release_add(255);
#[test]
fn checked_boundaries() {
    assert_eq!(STATIC, 255);
    assert_eq!(Owner::VALUE, 255);
    assert_eq!(ANON.len(), 2);
    let inline = const { let x = 254u8; x + 1 };
    assert_eq!(inline, 255);
    assert_eq!(U8, 255);
    assert_eq!(I8, -128);
    assert_eq!(NEG_LITERAL, -128);
    assert_eq!(U64, 18446744065119617025);
    assert_eq!(I64, -21);
    assert_eq!(U128, (1u128 << 100) * 3);
    assert_eq!(I128, -(1i128 << 100) * 3);
    assert_eq!(I128_MIN, i128::MIN);
    assert_eq!(I128_SIGN, i128::MAX);
    assert_eq!(SHIFT, 128);
    assert_eq!(MASKED, 0);
    assert_eq!(OVERFLOWING, (0, true));
    assert_eq!(BOOL, true);
    assert_eq!(CAST, 44);
    assert_eq!(RELEASE, 0);
}
