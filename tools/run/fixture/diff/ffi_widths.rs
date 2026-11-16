// diff_fast corpus program: the widths and signedness of core::ffi's C scalar types on this target, as
// `D <type> <bytes> <signed>`. `long` is 64 bits only on 64-bit targets other than Windows (LLP64): on
// Windows a C long is an i32, which bindgen output for Windows APIs relies on (`let e: c_long = 0i32`).
// rustc --test -O and `makepad test` (all tiers) must print identical D lines.
use std::ffi::*;
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
macro_rules! width {
    ($($t:ident)*) => {$( d!("{} {} {}", stringify!($t), std::mem::size_of::<$t>(), <$t>::MIN != 0); )*};
}

#[test]
fn run() {
    width!(c_char c_schar c_uchar c_short c_ushort c_int c_uint c_long c_ulong c_longlong c_ulonglong);
    d!("c_float {} c_double {}", std::mem::size_of::<c_float>(), std::mem::size_of::<c_double>());
    #[cfg(windows)]
    {
        let (e, f): (c_long, c_ulong) = (-1i32, u32::MAX);
        d!("windows long {e} {f}");
    }
}
