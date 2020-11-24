
#[macro_export]
macro_rules! class {
    ($name:ident) => ({
        #[allow(deprecated)]
        #[inline(always)]
        fn get_class(name: &str) -> Option<&'static $crate::runtime::Class> {
            unsafe {
                #[cfg_attr(feature = "cargo-clippy", allow(replace_consts))]
                static CLASS: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::ATOMIC_USIZE_INIT;
                // `Relaxed` should be fine since `objc_getClass` is thread-safe.
                let ptr = CLASS.load(::std::sync::atomic::Ordering::Relaxed) as *const $crate::runtime::Class;
                if ptr.is_null() {
                    let cls = $crate::runtime::objc_getClass(name.as_ptr() as *const _);
                    CLASS.store(cls as usize, ::std::sync::atomic::Ordering::Relaxed);
                    if cls.is_null() { None } else { Some(&*cls) }
                } else {
                    Some(&*ptr)
                }
            }
        }
        match get_class(concat!(stringify!($name), '\0')) {
            Some(cls) => cls,
            None => panic!("Class with name {} could not be found", stringify!($name)),
        }
    })
}

#[doc(hidden)]
#[macro_export]
macro_rules! sel_impl {
    // Declare a function to hide unsafety, otherwise we can trigger the
    // unused_unsafe lint; see rust-lang/rust#8472
    ($name:expr) => ({
        #[allow(deprecated)]
        #[inline(always)]
        fn register_sel(name: &str) -> $crate::runtime::Sel {
            unsafe {
                #[cfg_attr(feature = "cargo-clippy", allow(replace_consts))]
                static SEL: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::ATOMIC_USIZE_INIT;
                let ptr = SEL.load(::std::sync::atomic::Ordering::Relaxed) as *const ::std::os::raw::c_void;
                // It should be fine to use `Relaxed` ordering here because `sel_registerName` is
                // thread-safe.
                if ptr.is_null() {
                    let sel = $crate::runtime::sel_registerName(name.as_ptr() as *const _);
                    SEL.store(sel.as_ptr() as usize, ::std::sync::atomic::Ordering::Relaxed);
                    sel
                } else {
                    $crate::runtime::Sel::from_ptr(ptr)
                }
            }
        }
        register_sel($name)
    })
}

#[macro_export]
macro_rules! sel {
    ($name:ident) => ({sel_impl!(concat!(stringify!($name), '\0'))});
    ($($name:ident :)+) => ({sel_impl!(concat!($(stringify!($name), ':'),+, '\0'))});
}


#[macro_export]
macro_rules! msg_send {
    (super($obj:expr, $superclass:expr), $name:ident) => ({
        let sel = sel!($name);
        let result;
        match $crate::__send_super_message(&*$obj, $superclass, sel, ()) {
            Err(s) => panic!("{}", s),
            Ok(r) => result = r,
        }
        result
    });
    (super($obj:expr, $superclass:expr), $($name:ident : $arg:expr)+) => ({
        let sel = sel!($($name:)+);
        let result;
        match $crate::__send_super_message(&*$obj, $superclass, sel, ($($arg,)*)) {
            Err(s) => panic!("{}", s),
            Ok(r) => result = r,
        }
        result
    });
    ($obj:expr, $name:ident) => ({
        let sel = sel!($name);
        let result;
        match $crate::__send_message(&*$obj, sel, ()) {
            Err(s) => panic!("{}", s),
            Ok(r) => result = r,
        }
        result
    });
    ($obj:expr, $($name:ident : $arg:expr)+) => ({
        let sel = sel!($($name:)+);
        let result;
        match $crate::__send_message(&*$obj, sel, ($($arg,)*)) {
            Err(s) => panic!("{}", s),
            Ok(r) => result = r,
        }
        result
    });
}
