//! A `vis` fragment that matched nothing is still a fragment where it is forwarded: matched again,
//! `$v:vis` takes it, also right before a closing delimiter or at the end of the input, where a
//! `vis` written as nothing does not match (relax issue 26; rustc forwards it as an empty invisible
//! group). The shape is a pin-projection macro's: the item's visibility goes on as the projection's
//! (`[$vis $kind $name $vis]`), and on again in a group of its own (`[$proj_vis]`). A call whose doc
//! comments are desugared has its tokens out of text order; an empty `vis` forwarded into it is
//! still taken, and one forwarded earlier in the file does not upset its match (relax issue 30).
macro_rules! projected {
    ($($tt:tt)*) => {
        projected_internal! { [] $($tt)* }
    };
}

macro_rules! projected_internal {
    ([$($attrs:tt)*] #[$($attr:tt)*] $($tt:tt)*) => {
        projected_internal! { [$($attrs)* #[$($attr)*]] $($tt)* }
    };
    ([$($attrs:tt)*] pub $kind:ident $name:ident $($tt:tt)*) => {
        projected_generics! { [$($attrs)*] [pub $kind $name pub(crate)] $($tt)* }
    };
    ([$($attrs:tt)*] $vis:vis $kind:ident $name:ident $($tt:tt)*) => {
        projected_generics! { [$($attrs)*] [$vis $kind $name $vis] $($tt)* }
    };
}

macro_rules! projected_generics {
    (
        [$($attrs:tt)*]
        [$vis:vis $kind:ident $name:ident $proj_vis:vis]
        $(<
            $( $lifetime:lifetime $(: $lifetime_bound:lifetime)? ),* $(,)?
            $( $generics:ident $(: $generics_bound:path)? ),* $(,)?
        >)?
        {
            $( $(#[$pin:ident])? $field_vis:vis $field:ident : $field_ty:ty ),+ $(,)?
        }
    ) => {
        projected_expand! {
            [$proj_vis]
            [$($attrs)* $vis $kind $name]
            [$(< $( $lifetime $(: $lifetime_bound)? ,)* $( $generics $(: $generics_bound)? ),* >)?]
            [$(< $( $lifetime ,)* $( $generics ),* >)?]
            { $( [$($pin)?] $field_vis $field : $field_ty ),+ }
        }
    };
}

macro_rules! projected_expand {
    (
        [$proj_vis:vis]
        [$(#[$attrs:meta])* $vis:vis struct $name:ident]
        [$($def_generics:tt)*]
        [$($ty_generics:tt)*]
        { $( [$($pin:ident)?] $field_vis:vis $field:ident : $field_ty:ty ),+ }
    ) => {
        $(#[$attrs])* $vis struct $name $($def_generics)* { $( $field_vis $field : $field_ty ),+ }
        impl $($def_generics)* $name $($ty_generics)* {
            $proj_vis fn pinned(&self) -> usize { 0 $( + projected_expand!(@pin [$($pin)?]) )+ }
        }
    };
    (@pin []) => { 0 };
    (@pin [pin]) => { 1 };
}

projected! {
    struct Two<T, U> {
        #[pin]
        a: T,
        b: U,
    }
}

projected! {
    #[derive(Clone, Copy)]
    struct Life<'a> {
        #[pin]
        r: &'a u8,
    }
}

projected! {
    pub struct Wrapper<T: Copy> {
        #[pin]
        inner: T,
    }
}

projected! {
    struct Plain {
        a: u8,
        #[pin]
        b: u16,
        #[pin]
        c: u32,
    }
}

// forwarded at the end of the input
macro_rules! made_with {
    ($v:vis) => {
        $v fn made() -> u8 { 7 }
    };
}

macro_rules! forward_vis {
    ($v:vis $name:ident) => {
        made_with!($v);
    };
}

forward_vis!(unused_name);

// a doc comment of the macro's body, desugared where the next call is matched, before the
// forwarded empty `vis` at the end of the input
macro_rules! documented {
    ($v:vis $name:ident) => {
        documented_inner! {
            $name
            /// a unit struct made through a forwarded empty visibility
            $v
        }
    };
}

macro_rules! documented_inner {
    ($name:ident $(#[$attr:meta])* $v:vis) => {
        $(#[$attr])* $v struct $name;
        impl $name {
            fn tag(&self) -> u8 { 9 }
        }
    };
}

documented!(Documented);

// a call's own doc comment, after empty `vis` fragments were forwarded above (a thread-local
// declaration's shape)
macro_rules! keyed {
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr;) => {
        keyed_inner!($(#[$attr])* $vis $name, $t, $init);
    };
}

macro_rules! keyed_inner {
    ($(#[$attr:meta])* $vis:vis $name:ident, $t:ty, $init:expr) => {
        $(#[$attr])* $vis static $name: $t = $init;
    };
}

keyed! { static FIRST: u32 = 1; }

keyed! {
    /// documented, after the empty visibilities forwarded above
    static SECOND: u32 = 2;
}

// a list of declarations, each passed on with the rest after it: the second one's doc comment
// goes on desugared, after the first one's empty `vis`
macro_rules! keyed_list {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr; $($rest:tt)*) => {
        keyed_inner!($(#[$attr])* $vis $name, $t, $init);
        keyed_list!($($rest)*);
    };
}

keyed_list! {
    static THIRD: u32 = 3;
    /// documented, the second of the list
    static FOURTH: u32 = 4;
}

#[test]
fn forwarded_empty_vis_fragments() {
    let two = Two { a: 1u8, b: 2u16 };
    assert_eq!((two.pinned(), two.a, two.b), (1, 1, 2), "two type parameters");
    let byte = 3u8;
    let life = Life { r: &byte };
    let copy = life;
    assert_eq!((life.pinned(), *copy.r), (1, 3), "a lifetime, and a derive passed on");
    let wrapper = Wrapper { inner: 4u32 };
    assert_eq!((wrapper.pinned(), wrapper.inner), (1, 4), "pub, one type parameter");
    let plain = Plain { a: 5, b: 6, c: 7 };
    assert_eq!((plain.pinned(), plain.a, plain.b, plain.c), (2, 5, 6, 7), "no generics");
    assert_eq!(made(), 7, "an empty vis forwarded as the whole input");
    assert_eq!(Documented.tag(), 9, "an empty vis at the end of a call whose doc comment is desugared");
    assert_eq!(FIRST + SECOND, 3, "a documented call after empty visibilities were forwarded");
    assert_eq!(THIRD + FOURTH, 7, "a documented declaration passed on after an empty visibility");
}
