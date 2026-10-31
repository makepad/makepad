// rand's float_impls: a captured ty ends before the following `<<` or `<`.
// rustc_parse parser/ty.rs: parse_ty_common; parser/mod.rs: eat_metavar_seq.
macro_rules! float_impls {
    ($name:ident, $f_scalar:ident, $u_scalar:ty, $fraction_bits:expr) => {
        fn $name() -> $f_scalar {
            let precision = $fraction_bits + 1;
            1.0 / ((1 as $u_scalar << precision) as $f_scalar)
        }
    };
}
float_impls!(scale32, f32, u32, 23);
float_impls!(scale64, f64, u64, 52);

macro_rules! optional_float_impls {
    ($($meta:meta)?, $name:ident, $f:ident, $u:ty) => {
        $(#[cfg($meta)])?
        fn $name() -> $f { 1.0 / ((1 as $u << 4) as $f) }
    };
}
optional_float_impls!(, scale_optional, f32, u32);
optional_float_impls!(all(), scale_enabled, f64, u64);
optional_float_impls!(any(), scale_disabled, MissingFloat, MissingInt);

macro_rules! emit {
    ($($tokens:tt)*) => { $($tokens)* };
}
macro_rules! forwarded {
    ($t:ty) => {
        emit!({
            let shift = 1 as $t << 3;
            let less = 1 as $t < 2;
            (shift, less)
        })
    };
}
type Word = u32;

#[test]
fn captured_type_ends_before_shift() {
    assert_eq!(scale32(), 1.0f32 / 16_777_216.0);
    assert_eq!(scale64(), 1.0f64 / 9_007_199_254_740_992.0);
    assert_eq!(scale_optional(), 0.0625);
    assert_eq!(scale_enabled(), 0.0625);
    assert_eq!(forwarded!(u32), (8, true));
    assert_eq!(forwarded!(crate::Word), (8, true));
}
