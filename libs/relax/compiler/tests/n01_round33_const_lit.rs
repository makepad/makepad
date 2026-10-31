// typeck-audit 1.17 / front-bck 1.11: const-argument literals as hir_ty_lowering
// try_lower_anon_const_lit + lit_to_const (suffix must match, negation, block unwrap).
struct U8<const N: u8>;
impl<const N: u8> U8<N> { const V: u8 = N; }
struct I32<const N: i32>;
impl<const N: i32> I32<N> { const V: i32 = N; }
struct I8<const N: i8>;
impl<const N: i8> I8<N> { const V: i8 = N; }

fn own<const N: i32>(x: i32) -> i32 { x + N }
struct S;
impl S { fn m<const N: i32>(&self) -> i32 { N } }

// rustc_parse path.rs is_args_start: `<-` (LArrow) starts generic arguments and is split.
#[test]
fn unspaced_negative_turbofish() {
    assert_eq!(own::<-2>(5), 3);
    assert_eq!(S.m::<-4>(), -4);
    let _: I32<-1> = I32;
}

#[test]
fn literal_const_args() {
    assert_eq!(I32::<-3>::V, -3);
    assert_eq!(I32::<{ -3 }>::V, -3);
    assert_eq!(I32::<-3i32>::V, -3);
    assert_eq!(U8::<7u8>::V, 7);
    assert_eq!(U8::<{ 255 }>::V, 255);
    assert_eq!(I8::<-128>::V, -128);
    assert_eq!(I8::<{ -128i8 }>::V, -128);
}
