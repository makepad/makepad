#[macro_export]
macro_rules! class {
    ($name:ident) => {{
        static CLASS: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);
        $crate::runtime::__cached_class(&CLASS, concat!(stringify!($name), '\0'))
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! sel_impl {
    ($name:expr) => {{
        static SEL: ::std::sync::atomic::AtomicUsize = ::std::sync::atomic::AtomicUsize::new(0);
        $crate::runtime::__cached_sel(&SEL, $name)
    }};
}

#[macro_export]
macro_rules! sel {
    ($name:ident) => ({sel_impl!(concat!(stringify!($name), '\0'))});
    ($($name:ident :)+) => ({sel_impl!(concat!($(stringify!($name), ':'),+, '\0'))});
}

#[macro_export]
macro_rules! msg_send {
    (super($obj:expr, $superclass:expr), $name:ident) => ({
        $crate::__send_super_message_or_panic(&*$obj, $superclass, sel!($name), ())
    });
    (super($obj:expr, $superclass:expr), $($name:ident : $arg:expr)+) => ({
        $crate::__send_super_message_or_panic(&*$obj, $superclass, sel!($($name:)+), ($($arg,)*))
    });
    ($obj:expr, $name:ident) => ({
        $crate::__send_message_or_panic(&*$obj, sel!($name), ())
    });
    ($obj:expr, $($name:ident : $arg:expr)+) => ({
        $crate::__send_message_or_panic(&*$obj, sel!($($name:)+), ($($arg,)*))
    });
}
