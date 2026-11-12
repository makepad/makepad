//! The declaration crate imports the entry point by its hack name (unic-langid-macros:
//! `unic_langid_macros_impl::_proc_macro_hack_langid`) and exports the call macro the way
//! proc-macro-hack writes it: a derive on an enum, then the `proc_macro_call!` the derive defines.
//! The derive's output reaches this crate's items through `$crate` (`$crate::subtags::Language`).
#[doc(hidden)]
pub use pmh_imp::_proc_macro_hack_answer;

/// What the macro's output names through `$crate`.
pub const ANSWER: u32 = 42;

#[macro_export]
macro_rules! answer {
    ($($input:tt)*) => {{
        #[derive($crate::_proc_macro_hack_answer)]
        #[allow(dead_code)]
        enum ProcMacroHack {
            Value = (stringify!($($input)*), 0).1,
        }
        proc_macro_call!()
    }};
}
