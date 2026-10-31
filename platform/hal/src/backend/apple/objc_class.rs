//! Runtime method tables shared by the Apple accessibility adapters. `ClassDecl::add_method`
//! derives the Objective-C encoding from the same typed function pointer as the thunk.

pub(crate) fn shielded<R>(name: &str, fallback: impl FnOnce() -> R, call: impl FnOnce() -> R) -> R {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).unwrap_or_else(|_| {
        eprintln!("makepad-hal: PANIC contained at the {name} accessibility callback boundary");
        fallback()
    })
}

macro_rules! objc_class {
    ($decl:ident; $( ($($selector:tt)*) => $method:ident($($arg:ident: $ty:ty),*) -> $ret:ty = $fallback:expr; )*) => {
        $( {
            extern "C" fn thunk(this: &makepad_apple_sys::Object, _: makepad_apple_sys::Sel, $($arg: $ty),*) -> $ret {
                $crate::backend::apple::objc_class::shielded(
                    stringify!($($selector)*), || $fallback, || $method(this, $($arg),*)
                )
            }
            // ClassDecl generates and installs the encoding from this signature; no independent
            // encoding table can drift from the ABI, including BOOL and struct returns.
            unsafe {
                $decl.add_method(makepad_apple_sys::sel!($($selector)*),
                    thunk as extern "C" fn(&makepad_apple_sys::Object, makepad_apple_sys::Sel, $($ty),*) -> $ret);
            }
        } )*
    };
}

pub(crate) use objc_class;
