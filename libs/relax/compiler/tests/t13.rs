// Relax: macro_rules! -- fragments, repetitions, recursion, item generation, hygiene of locals.
macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}
macro_rules! sum {
    () => { 0 };
    ($h:expr $(, $t:expr)*) => { $h + sum!($($t),*) };
}
macro_rules! make_struct {
    ($name:ident { $($f:ident : $t:ty),* $(,)? }) => {
        struct $name { $($f: $t),* }
        impl $name {
            fn total(&self) -> i64 { 0 $(+ self.$f as i64)* }
        }
    };
}
macro_rules! swap_let {
    ($a:ident, $b:ident) => {
        let tmp = $a;
        $a = $b;
        $b = tmp;
    };
}
make_struct!(P { a: u32, b: i16, });
#[test]
fn macros() {
    assert!(square!(3 + 1) == 16);
    assert!(sum!(1, 2, 3, 4) == 10);
    let p = P { a: 5, b: -2 };
    assert!(p.total() == 3);
    let mut x = 1;
    let mut y = 2;
    let tmp = 99;
    swap_let!(x, y);
    assert!(x == 2 && y == 1 && tmp == 99);
}
macro_rules! flags {
    (
        $(#[$outer:meta])*
        $vis:vis struct $BitFlags:ident: $T:ty {
            $(
                $(#[$inner:ident $($args:tt)*])*
                const $Flag:tt = $value:expr;
            )*
        }
        $($t:tt)*
    ) => {
        $(#[$outer])*
        $vis struct $BitFlags($T);
        impl $BitFlags {
            $(
                pub const $Flag: $BitFlags = $BitFlags($value);
            )*
            pub fn bits(&self) -> $T { self.0 }
        }
    };
}
flags! {
    /// doc
    #[derive(Copy, Clone, PartialEq)]
    #[doc(alias = "click")]
    pub struct MouseButton: u32 {
        /// primary
        const PRIMARY = 1 << 0;
        #[doc = "x"]
        const SECONDARY = 1 << 1;
    }
}
#[test]
fn bitflags_like() { assert!(MouseButton::SECONDARY.bits() == 2 && MouseButton::PRIMARY == MouseButton::PRIMARY); }
