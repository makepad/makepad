// rustc_expand mbe/quoted.rs: parse_tree represents `$crate` as one Ident.
// rustc_parse parser/nonterminal.rs: get_macro_ident accepts that token.
macro_rules! declare_callback {
    ($d:tt) => {
        macro_rules! pg_cfg_expansion {
            (
                ($d($callback:ident)::* !($d($args:tt)*) $d($rem:tt)*)
                $gen_args:tt [] $generics:tt [] $deleted:tt []
            ) => {
                $d($callback)::* ! {
                    $d($args)* $gen_args $generics $deleted $d($rem)*
                }
            };
        }
    };
}
declare_callback!($);

trait Kind { type To<T: ?Sized>: ?Sized; }
impl Kind for () { type To<T: ?Sized> = T; }
trait TypeFn<K> { type Output: ?Sized; }

#[macro_export]
macro_rules! finish_type_fn {
    (
        struct $name:ident<$t:ident: ?Sized>; [] [] []
        impl<$k:ident: $bound:ident> $arg:ty => $ret:ty
    ) => {
        struct $name<$t: ?Sized>(core::marker::PhantomData<$t>);
        impl<$t: ?Sized, $k: $bound> TypeFn<$arg> for $name<$t> {
            type Output = $ret;
        }
    };
}

macro_rules! type_fn {
    (struct $name:ident<$t:ident: ?Sized>; $($rest:tt)*) => {
        pg_cfg_expansion! {
            ($crate::finish_type_fn!(struct $name<$t: ?Sized>;) $($rest)*)
            [] [] [] [] [] []
        }
    };
}
type_fn! {
    struct CoerceToFn<T: ?Sized>;
    impl<K: Kind> K => <K as Kind>::To<T>
}

macro_rules! one_tt {
    ($t:tt) => { 1 };
    ($($t:tt)*) => { 2 };
}
macro_rules! crate_token {
    () => { one_tt!($crate) };
}

const VALUE: u32 = 23;
macro_rules! crate_value {
    () => { $crate::VALUE };
}
macro_rules! crate_import {
    () => { use $crate::VALUE as IMPORTED; };
}
mod no_prelude {
    #![no_implicit_prelude]
    crate_import!();
    pub fn value() -> u32 { crate_value!() + IMPORTED }
}

#[test]
fn generated_callback_accepts_dollar_crate() {
    let value: <CoerceToFn<u32> as TypeFn<()>>::Output = 17;
    let slice: &<CoerceToFn<[u8]> as TypeFn<()>>::Output = &[3, 5];
    assert_eq!(value, 17);
    assert_eq!(slice.len(), 2);
    assert_eq!(crate_token!(), 1);
    assert_eq!(no_prelude::value(), 46);
}
