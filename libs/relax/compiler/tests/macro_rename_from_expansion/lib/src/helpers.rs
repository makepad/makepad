// icu_locale_core/src/helpers.rs, reduced: a validated string type and, for it, a macro under
// an internal `#[macro_export]` name that the same expansion re-exports under the public one.
macro_rules! impl_tinystr_subtag {
    ($name:ident, $($path:ident)::+, $macro_name:ident, $internal_macro_name:ident, $len:expr) => {
        #[derive(Debug, PartialEq, Clone, Copy)]
        pub struct $name(pub &'static str);

        impl $name {
            pub const fn try_from_str(s: &'static str) -> Result<Self, ()> {
                if s.len() >= $len { Ok(Self(s)) } else { Err(()) }
            }
        }

        #[macro_export]
        #[doc(hidden)]
        macro_rules! $internal_macro_name {
            ($string:literal) => { const {
                use $crate::$($path ::)+ $name;
                match $name::try_from_str($string) {
                    Ok(r) => r,
                    _ => panic!(concat!("Invalid ", stringify!($name), ": ", $string)),
                }
            }};
        }
        #[doc(inline)]
        pub use $internal_macro_name as $macro_name;
    };
}
