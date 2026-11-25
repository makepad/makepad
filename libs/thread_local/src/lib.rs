//! Thread-local storage for Makepad and Relax.
//! Native Android code shares one pthread key across its thread-locals so
//! packed apps can fit within bionic's process-wide key limit.
//! Other targets and code compiled by Relax use std's thread-locals.

#[cfg(not(all(target_os = "android", not(relax))))]
pub use std::thread::{AccessError, LocalKey};

#[cfg(not(all(target_os = "android", not(relax))))]
/// Declares thread-local keys using std's syntax.
#[macro_export]
macro_rules! thread_local {
    ($($t:tt)*) => { ::std::thread_local! { $($t)* } };
}

#[cfg(any(all(target_os = "android", not(relax)), all(test, unix)))]
#[doc(hidden)]
pub mod table;

#[cfg(all(target_os = "android", not(relax)))]
pub use table::{AccessError, LocalKey};

#[cfg(all(target_os = "android", not(relax)))]
/// Declares thread-local keys that share the native Android table. A macro of its own, not a
/// `pub use` of the table's: `#[macro_use(thread_local)] extern crate` imports exported macros.
#[macro_export]
macro_rules! thread_local {
    ($($t:tt)*) => { $crate::__table_thread_local! { $($t)* } };
}

#[cfg(any(all(target_os = "android", not(relax)), all(test, unix)))]
#[doc(hidden)]
#[macro_export]
macro_rules! __table_thread_local {
    () => {};
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = const $init:block $(; $($rest:tt)*)?) => {
        $crate::__table_thread_local! {
            $(#[$attr])* $vis static $name: $t = { const $init } $(; $($rest)*)?
        }
    };
    ($(#[$attr:meta])* $vis:vis static $name:ident: $t:ty = $init:expr $(; $($rest:tt)*)?) => {
        $(#[$attr])* $vis const $name: $crate::table::LocalKey<$t> = {
            static __MAKEPAD_SLOT: $crate::table::Slot<$t> = $crate::table::Slot::new();
            #[inline]
            fn __makepad_init() -> $t { $init }
            $crate::table::LocalKey::__new(&__MAKEPAD_SLOT, __makepad_init)
        };
        $($crate::__table_thread_local! { $($rest)* })?
    };
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DROPS: AtomicUsize = AtomicUsize::new(0);
    struct Counted;
    impl Drop for Counted {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    crate::thread_local! {
        /// Attributes and visibility pass through.
        #[allow(dead_code)]
        pub(crate) static COUNT: Cell<u32> = const { Cell::new(0) };
        static LOG: RefCell<Vec<u32>> = RefCell::new(vec![1]);
        static COUNTED: RefCell<Option<Counted>> = const { RefCell::new(None) }
    }
    crate::thread_local!(static ONE: Cell<u8> = Cell::new(1));

    /// The macro every host crate's `thread_local!` resolves to: off native Android it is std's,
    /// with std's key type, and std's call forms (several statics, `const`, attributes, a
    /// trailing `;` or none, the `()` form) all declare per-thread values dropped at thread exit.
    #[test]
    fn forwards_to_std_off_android() {
        #[cfg(not(target_os = "android"))]
        {
            let _: &'static std::thread::LocalKey<Cell<u32>> = &COUNT;
            let _: Option<std::thread::AccessError> = LOG.try_with(|_| ()).err();
        }
        COUNT.set(5);
        LOG.with_borrow_mut(|l| l.push(2));
        std::thread::spawn(|| {
            assert_eq!(COUNT.get(), 0, "each thread starts from the initial value");
            LOG.with_borrow(|l| assert_eq!(*l, [1], "each thread starts from the initial value"));
            COUNTED.set(Some(Counted));
        })
        .join()
        .unwrap();
        assert_eq!(DROPS.load(Ordering::Relaxed), 1, "a thread's value drops at its exit");
        assert_eq!((COUNT.get(), ONE.get()), (5, 1), "this thread's values are its own");
        LOG.with_borrow(|l| assert_eq!(*l, [1, 2]));
    }
}
