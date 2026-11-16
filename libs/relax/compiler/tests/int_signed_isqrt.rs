// relax issue 231: the signed integers' `isqrt` and `checked_isqrt` (stable since Rust 1.84) were
// missing; only the unsigned ones and NonZero's were there.
#[test]
fn signed_integers_take_their_square_root() {
    assert_eq!((0i8.isqrt(), 1i8.isqrt(), i8::MAX.isqrt()), (0, 1, 11));
    assert_eq!((17i16.isqrt(), i16::MAX.isqrt()), (4, 181));
    assert_eq!((1_000_000i32.isqrt(), i32::MAX.isqrt()), (1000, 46340));
    assert_eq!((99i64.isqrt(), i64::MAX.isqrt()), (9, 3037000499));
    assert_eq!(((1i128 << 100).isqrt(), i128::MAX.isqrt()), (1 << 50, 13043817825332782212));
    assert_eq!(80isize.isqrt(), 8);
    assert_eq!(((-1i32).checked_isqrt(), 26i64.checked_isqrt(), i8::MIN.checked_isqrt()), (None, Some(5), None));
    assert_eq!(((-5i128).checked_isqrt(), 0i16.checked_isqrt()), (None, Some(0)));
    const C: i32 = 50i32.isqrt();
    assert_eq!(C, 7);
}

#[test]
fn a_negative_argument_panics() {
    let r = std::panic::catch_unwind(|| std::hint::black_box(-4i64).isqrt());
    let payload = r.unwrap_err();
    let msg = payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(|s| s.as_str()));
    assert_eq!(msg, Some("argument of integer square root cannot be negative"));
}
