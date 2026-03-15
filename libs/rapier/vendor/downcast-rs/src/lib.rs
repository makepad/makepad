#![deny(unsafe_code,rustdoc::bare_urls)]
#![cfg_attr(not(feature = "std"), no_std)]
//! [![Build status](https://img.shields.io/github/actions/workflow/status/marcianx/downcast-rs/main.yml?branch=master)](https://github.com/marcianx/downcast-rs/actions)
//! [![Latest version](https://img.shields.io/crates/v/downcast-rs.svg)](https://crates.io/crates/downcast-rs)
//! [![Documentation](https://docs.rs/downcast-rs/badge.svg)](https://docs.rs/downcast-rs)
//!
//! Rust enums are great for types where all variations are known beforehand. But a
//! container of user-defined types requires an open-ended type like a **trait
//! object**. Some applications may want to cast these trait objects back to the
//! original concrete types to access additional functionality and performant
//! inlined implementations.
//!
//! `downcast-rs` adds this downcasting support to trait objects using only safe
//! Rust. It supports **type parameters**, **associated types**, and **constraints**.
//!
//! # Usage
//!
//! Add the following to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! downcast-rs = "2.0.1"
//! ```
//!
//! This crate is `no_std` compatible. To use it without `std`:
//!
//! ```toml
//! [dependencies]
//! downcast-rs = { version = "2.0.1", default-features = false }
//! ```
//!
//! To make a trait downcastable, make it extend either `Downcast` or `DowncastSync`
//! and invoke `impl_downcast!` on it as in the examples below.
//!
//! Since 2.0.0, the minimum supported Rust version is 1.56.
//!
//! ```
//! # use downcast_rs::{Downcast, impl_downcast};
//! # #[cfg(feature = "sync")]
//! # use downcast_rs::DowncastSync;
//! trait Trait: Downcast {}
//! impl_downcast!(Trait);
//!
//! // Also supports downcasting `Arc`-ed trait objects by extending `DowncastSync`
//! // and starting `impl_downcast!` with `sync`.
//! # #[cfg(feature = "sync")]
//! trait TraitSync: DowncastSync {}
//! # #[cfg(feature = "sync")]
//! impl_downcast!(sync TraitSync);
//!
//! // With type parameters.
//! trait TraitGeneric1<T>: Downcast {}
//! impl_downcast!(TraitGeneric1<T>);
//!
//! // With associated types.
//! trait TraitGeneric2: Downcast { type G; type H; }
//! impl_downcast!(TraitGeneric2 assoc G, H);
//!
//! // With constraints on types.
//! trait TraitGeneric3<T: Copy>: Downcast {
//!     type H: Clone;
//! }
//! impl_downcast!(TraitGeneric3<T> assoc H where T: Copy, H: Clone);
//!
//! // With concrete types.
//! trait TraitConcrete1<T: Copy>: Downcast {}
//! impl_downcast!(concrete TraitConcrete1<u32>);
//!
//! trait TraitConcrete2<T: Copy>: Downcast { type H; }
//! impl_downcast!(concrete TraitConcrete2<u32> assoc H=f64);
//! # fn main() {}
//! ```
//!
//! # Example without generics
//!
//! ```
//! # use std::rc::Rc;
//! # #[cfg(feature = "sync")]
//! # use std::sync::Arc;
//! # use downcast_rs::impl_downcast;
//! # #[cfg(not(feature = "sync"))]
//! # use downcast_rs::Downcast;
//! # #[cfg(feature = "sync")]
//! use downcast_rs::DowncastSync;
//!
//! // To create a trait with downcasting methods, extend `Downcast` or `DowncastSync`
//! // and run `impl_downcast!()` on the trait.
//! # #[cfg(not(feature = "sync"))]
//! # trait Base: Downcast {}
//! # #[cfg(not(feature = "sync"))]
//! # impl_downcast!(Base);
//! # #[cfg(feature = "sync")]
//! trait Base: DowncastSync {}
//! # #[cfg(feature = "sync")]
//! impl_downcast!(sync Base);  // `sync` => also produce `Arc` downcasts.
//!
//! // Concrete types implementing Base.
//! #[derive(Debug)]
//! struct Foo(u32);
//! impl Base for Foo {}
//! #[derive(Debug)]
//! struct Bar(f64);
//! impl Base for Bar {}
//!
//! fn main() {
//!     // Create a trait object.
//!     let mut base: Box<dyn Base> = Box::new(Foo(42));
//!
//!     // Try sequential downcasts.
//!     if let Some(foo) = base.downcast_ref::<Foo>() {
//!         assert_eq!(foo.0, 42);
//!     } else if let Some(bar) = base.downcast_ref::<Bar>() {
//!         assert_eq!(bar.0, 42.0);
//!     }
//!
//!     assert!(base.is::<Foo>());
//!
//!     // Fail to convert `Box<dyn Base>` into `Box<Bar>`.
//!     let res = base.downcast::<Bar>();
//!     assert!(res.is_err());
//!     let base = res.unwrap_err();
//!     // Convert `Box<dyn Base>` into `Box<Foo>`.
//!     assert_eq!(42, base.downcast::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);
//!
//!     // Also works with `Rc`.
//!     let mut rc: Rc<dyn Base> = Rc::new(Foo(42));
//!     assert_eq!(42, rc.downcast_rc::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);
//!
//!     // Since this trait is `Sync`, it also supports `Arc` downcasts.
//!     # #[cfg(feature = "sync")]
//!     let mut arc: Arc<dyn Base> = Arc::new(Foo(42));
//!     # #[cfg(feature = "sync")]
//!     assert_eq!(42, arc.downcast_arc::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);
//! }
//! ```
//!
//! # Example with a generic trait with associated types and constraints
//!
//! ```
//! use downcast_rs::{Downcast, impl_downcast};
//!
//! // To create a trait with downcasting methods, extend `Downcast` or `DowncastSync`
//! // and run `impl_downcast!()` on the trait.
//! trait Base<T: Clone>: Downcast { type H: Copy; }
//! impl_downcast!(Base<T> assoc H where T: Clone, H: Copy);
//! // or: impl_downcast!(concrete Base<u32> assoc H=f32)
//!
//! // Concrete types implementing Base.
//! struct Foo(u32);
//! impl Base<u32> for Foo { type H = f32; }
//! struct Bar(f64);
//! impl Base<u32> for Bar { type H = f32; }
//!
//! fn main() {
//!     // Create a trait object.
//!     let mut base: Box<dyn Base<u32, H=f32>> = Box::new(Bar(42.0));
//!
//!     // Try sequential downcasts.
//!     if let Some(foo) = base.downcast_ref::<Foo>() {
//!         assert_eq!(foo.0, 42);
//!     } else if let Some(bar) = base.downcast_ref::<Bar>() {
//!         assert_eq!(bar.0, 42.0);
//!     }
//!
//!     assert!(base.is::<Bar>());
//! }
//! ```

// for compatibility with no std and macros
#[doc(hidden)]
#[cfg(not(feature = "std"))]
pub extern crate core as __std;
#[doc(hidden)]
#[cfg(feature = "std")]
pub extern crate std as __std;
#[doc(hidden)]
pub extern crate alloc as __alloc;

use __std::any::Any;
use __alloc::{boxed::Box, rc::Rc};

#[cfg(feature = "sync")]
use __alloc::sync::Arc;

/// Supports conversion to `Any`. Traits to be extended by `impl_downcast!` must extend `Downcast`.
pub trait Downcast: Any {
    /// Converts `Box<dyn Trait>` (where `Trait: Downcast`) to `Box<dyn Any>`, which can then be
    /// `downcast` into `Box<dyn ConcreteType>` where `ConcreteType` implements `Trait`.
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    /// Converts `Rc<Trait>` (where `Trait: Downcast`) to `Rc<Any>`, which can then be further
    /// `downcast` into `Rc<ConcreteType>` where `ConcreteType` implements `Trait`.
    fn into_any_rc(self: Rc<Self>) -> Rc<dyn Any>;
    /// Converts `&Trait` (where `Trait: Downcast`) to `&Any`. This is needed since Rust cannot
    /// generate `&Any`'s vtable from `&Trait`'s.
    fn as_any(&self) -> &dyn Any;
    /// Converts `&mut Trait` (where `Trait: Downcast`) to `&Any`. This is needed since Rust cannot
    /// generate `&mut Any`'s vtable from `&mut Trait`'s.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> Downcast for T {
    fn into_any(self: Box<Self>) -> Box<dyn Any> { self }
    fn into_any_rc(self: Rc<Self>) -> Rc<dyn Any> { self }
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

/// Extends `Downcast` for `Send` traits to support upcasting to `Box<dyn Any + Send>` as well.
pub trait DowncastSend: Downcast + Send {
    /// Converts `Box<Trait>` (where `Trait: DowncastSend`) to `Box<dyn Any + Send>`, which
    /// can then be `downcast` into `Box<ConcreteType>` where `ConcreteType` implements `Trait`.
    fn into_any_send(self: Box<Self>) -> Box<dyn Any + Send>;
}

impl<T: Any + Send> DowncastSend for T {
    fn into_any_send(self: Box<Self>) -> Box<dyn Any + Send> { self }
}

#[cfg(feature = "sync")]
/// Extends `DowncastSend` for `Sync` traits to support upcasting to `Box<dyn Any + Send + Sync>`
/// and `Arc` downcasting.
pub trait DowncastSync: DowncastSend + Sync {
    /// Converts `Box<Trait>` (where `Trait: DowncastSync`) to `Box<dyn Any + Send + Sync>`,
    /// which can then be `downcast` into `Box<ConcreteType>` where `ConcreteType` implements
    /// `Trait`.
    fn into_any_sync(self: Box<Self>) -> Box<dyn Any + Send + Sync>;
    /// Converts `Arc<Trait>` (where `Trait: DowncastSync`) to `Arc<Any>`, which can then be
    /// `downcast` into `Arc<ConcreteType>` where `ConcreteType` implements `Trait`.
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync>;
}

#[cfg(feature = "sync")]
impl<T: Any + Send + Sync> DowncastSync for T {
    fn into_any_sync(self: Box<Self>) -> Box<dyn Any + Send + Sync> { self }
    fn into_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> { self }
}

/// Adds downcasting support to traits that extend `Downcast` by defining forwarding
/// methods to the corresponding implementations on `std::any::Any` in the standard library.
///
/// See <https://users.rust-lang.org/t/how-to-create-a-macro-to-impl-a-provided-type-parametrized-trait/5289>
/// for why this is implemented this way to support templatized traits.
#[macro_export(local_inner_macros)]
macro_rules! impl_downcast {
    (@impl_full
        $trait_:ident [$($param_types:tt)*]
        for [$($forall_types:ident),*]
        where [$($preds:tt)*]
    ) => {
        impl_downcast! {
            @inject_where
                [impl<$($forall_types),*> dyn $trait_<$($param_types)*>]
                types [$($forall_types),*]
                where [$($preds)*]
                [{
                    impl_downcast! { @impl_body $trait_ [$($param_types)*] }
                }]
        }
    };

    (@impl_full_sync
        $trait_:ident [$($param_types:tt)*]
        for [$($forall_types:ident),*]
        where [$($preds:tt)*]
    ) => {
        impl_downcast! {
            @inject_where
                [impl<$($forall_types),*> dyn $trait_<$($param_types)*>]
                types [$($forall_types),*]
                where [$($preds)*]
                [{
                    impl_downcast! { @impl_body $trait_ [$($param_types)*] }
                    impl_downcast! { @impl_body_sync $trait_ [$($param_types)*] }
                }]
        }
    };

    (@impl_body $trait_:ident [$($types:tt)*]) => {
        /// Returns true if the trait object wraps an object of type `__T`.
        #[inline]
        pub fn is<__T: $trait_<$($types)*>>(&self) -> bool {
            $crate::Downcast::as_any(self).is::<__T>()
        }
        /// Returns a boxed object from a boxed trait object if the underlying object is of type
        /// `__T`. Returns the original boxed trait if it isn't.
        #[inline]
        pub fn downcast<__T: $trait_<$($types)*>>(
            self: $crate::__alloc::boxed::Box<Self>
        ) -> $crate::__std::result::Result<$crate::__alloc::boxed::Box<__T>, $crate::__alloc::boxed::Box<Self>> {
            if self.is::<__T>() {
                Ok($crate::Downcast::into_any(self).downcast::<__T>().unwrap())
            } else {
                Err(self)
            }
        }
        /// Returns an `Rc`-ed object from an `Rc`-ed trait object if the underlying object is of
        /// type `__T`. Returns the original `Rc`-ed trait if it isn't.
        #[inline]
        pub fn downcast_rc<__T: $trait_<$($types)*>>(
            self: $crate::__alloc::rc::Rc<Self>
        ) -> $crate::__std::result::Result<$crate::__alloc::rc::Rc<__T>, $crate::__alloc::rc::Rc<Self>> {
            if self.is::<__T>() {
                Ok($crate::Downcast::into_any_rc(self).downcast::<__T>().unwrap())
            } else {
                Err(self)
            }
        }
        /// Returns a reference to the object within the trait object if it is of type `__T`, or
        /// `None` if it isn't.
        #[inline]
        pub fn downcast_ref<__T: $trait_<$($types)*>>(&self) -> $crate::__std::option::Option<&__T> {
            $crate::Downcast::as_any(self).downcast_ref::<__T>()
        }
        /// Returns a mutable reference to the object within the trait object if it is of type
        /// `__T`, or `None` if it isn't.
        #[inline]
        pub fn downcast_mut<__T: $trait_<$($types)*>>(&mut self) -> $crate::__std::option::Option<&mut __T> {
            $crate::Downcast::as_any_mut(self).downcast_mut::<__T>()
        }
    };

    (@impl_body_sync $trait_:ident [$($types:tt)*]) => {
        /// Returns an `Arc`-ed object from an `Arc`-ed trait object if the underlying object is of
        /// type `__T`. Returns the original `Arc`-ed trait if it isn't.
        #[inline]
        pub fn downcast_arc<__T: $trait_<$($types)*> + $crate::__std::any::Any + $crate::__std::marker::Send + $crate::__std::marker::Sync>(
            self: $crate::__alloc::sync::Arc<Self>,
        ) -> $crate::__std::result::Result<$crate::__alloc::sync::Arc<__T>, $crate::__alloc::sync::Arc<Self>>
        {
            if self.is::<__T>() {
                Ok($crate::DowncastSync::into_any_arc(self).downcast::<__T>().unwrap())
            } else {
                Err(self)
            }
        }
    };

    (@inject_where [$($before:tt)*] types [] where [] [$($after:tt)*]) => {
        impl_downcast! { @as_item $($before)* $($after)* }
    };

    (@inject_where [$($before:tt)*] types [$($types:ident),*] where [] [$($after:tt)*]) => {
        impl_downcast! {
            @as_item
                $($before)*
                where $( $types: $crate::__std::any::Any + 'static ),*
                $($after)*
        }
    };
    (@inject_where [$($before:tt)*] types [$($types:ident),*] where [$($preds:tt)+] [$($after:tt)*]) => {
        impl_downcast! {
            @as_item
                $($before)*
                where
                    $( $types: $crate::__std::any::Any + 'static, )*
                    $($preds)*
                $($after)*
        }
    };

    (@as_item $i:item) => { $i };

    // No type parameters.
    ($trait_:ident   ) => { impl_downcast! { @impl_full $trait_ [] for [] where [] } };
    ($trait_:ident <>) => { impl_downcast! { @impl_full $trait_ [] for [] where [] } };
    (sync $trait_:ident   ) => { impl_downcast! { @impl_full_sync $trait_ [] for [] where [] } };
    (sync $trait_:ident <>) => { impl_downcast! { @impl_full_sync $trait_ [] for [] where [] } };
    // Type parameters.
    ($trait_:ident < $($types:ident),* >) => {
        impl_downcast! { @impl_full $trait_ [$($types),*] for [$($types),*] where [] }
    };
    (sync $trait_:ident < $($types:ident),* >) => {
        impl_downcast! { @impl_full_sync $trait_ [$($types),*] for [$($types),*] where [] }
    };
    // Type parameters and where clauses.
    ($trait_:ident < $($types:ident),* > where $($preds:tt)+) => {
        impl_downcast! { @impl_full $trait_ [$($types),*] for [$($types),*] where [$($preds)*] }
    };
    (sync $trait_:ident < $($types:ident),* > where $($preds:tt)+) => {
        impl_downcast! { @impl_full_sync $trait_ [$($types),*] for [$($types),*] where [$($preds)*] }
    };
    // Associated types.
    ($trait_:ident assoc $($atypes:ident),*) => {
        impl_downcast! { @impl_full $trait_ [$($atypes = $atypes),*] for [$($atypes),*] where [] }
    };
    (sync $trait_:ident assoc $($atypes:ident),*) => {
        impl_downcast! { @impl_full_sync $trait_ [$($atypes = $atypes),*] for [$($atypes),*] where [] }
    };
    // Associated types and where clauses.
    ($trait_:ident assoc $($atypes:ident),* where $($preds:tt)+) => {
        impl_downcast! { @impl_full $trait_ [$($atypes = $atypes),*] for [$($atypes),*] where [$($preds)*] }
    };
    (sync $trait_:ident assoc $($atypes:ident),* where $($preds:tt)+) => {
        impl_downcast! { @impl_full_sync $trait_ [$($atypes = $atypes),*] for [$($atypes),*] where [$($preds)*] }
    };
    // Type parameters and associated types.
    ($trait_:ident < $($types:ident),* > assoc $($atypes:ident),*) => {
        impl_downcast! {
            @impl_full
                $trait_ [$($types),*, $($atypes = $atypes),*]
                for [$($types),*, $($atypes),*]
                where []
        }
    };
    (sync $trait_:ident < $($types:ident),* > assoc $($atypes:ident),*) => {
        impl_downcast! {
            @impl_full_sync
                $trait_ [$($types),*, $($atypes = $atypes),*]
                for [$($types),*, $($atypes),*]
                where []
        }
    };
    // Type parameters, associated types, and where clauses.
    ($trait_:ident < $($types:ident),* > assoc $($atypes:ident),* where $($preds:tt)+) => {
        impl_downcast! {
            @impl_full
                $trait_ [$($types),*, $($atypes = $atypes),*]
                for [$($types),*, $($atypes),*]
                where [$($preds)*]
        }
    };
    (sync $trait_:ident < $($types:ident),* > assoc $($atypes:ident),* where $($preds:tt)+) => {
        impl_downcast! {
            @impl_full_sync
                $trait_ [$($types),*, $($atypes = $atypes),*]
                for [$($types),*, $($atypes),*]
                where [$($preds)*]
        }
    };
    // Concretely-parametrized types.
    (concrete $trait_:ident < $($types:ident),* >) => {
        impl_downcast! { @impl_full $trait_ [$($types),*] for [] where [] }
    };
    (sync concrete $trait_:ident < $($types:ident),* >) => {
        impl_downcast! { @impl_full_sync $trait_ [$($types),*] for [] where [] }
    };
    // Concretely-associated types types.
    (concrete $trait_:ident assoc $($atypes:ident = $aty:ty),*) => {
        impl_downcast! { @impl_full $trait_ [$($atypes = $aty),*] for [] where [] }
    };
    (sync concrete $trait_:ident assoc $($atypes:ident = $aty:ty),*) => {
        impl_downcast! { @impl_full_sync $trait_ [$($atypes = $aty),*] for [] where [] }
    };
    // Concretely-parametrized types with concrete associated types.
    (concrete $trait_:ident < $($types:ident),* > assoc $($atypes:ident = $aty:ty),*) => {
        impl_downcast! { @impl_full $trait_ [$($types),*, $($atypes = $aty),*] for [] where [] }
    };
    (sync concrete $trait_:ident < $($types:ident),* > assoc $($atypes:ident = $aty:ty),*) => {
        impl_downcast! { @impl_full_sync $trait_ [$($types),*, $($atypes = $aty),*] for [] where [] }
    };
}


#[cfg(all(test, feature = "sync"))]
mod test {
    /// Substitutes `Downcast` in the body with `DowncastSend`.
    macro_rules! subst_downcast_send {
        (@impl { $($parsed:tt)* }, {}) => { $($parsed)* };
        (@impl { $($parsed:tt)* }, { Downcast $($rest:tt)* }) => {
            subst_downcast_send!(@impl { $($parsed)* DowncastSend }, { $($rest)* });
        };
        (@impl { $($parsed:tt)* }, { $first:tt $($rest:tt)* }) => {
            subst_downcast_send!(@impl { $($parsed)* $first }, { $($rest)* });
        };
        ($($body:tt)+) => {
            subst_downcast_send!(@impl {}, { $($body)* });
        };
    }

    macro_rules! test_mod {
        (
            $test_mod_name:ident,
            trait $base_trait:path { $($base_impl:tt)* },
            non_sync: { $($non_sync_def:tt)+ },
            sync: { $($sync_def:tt)+ }
        ) => {
            test_mod! {
                $test_mod_name,
                trait $base_trait { $($base_impl:tt)* },
                type dyn $base_trait,
                non_sync: { $($non_sync_def)* },
                sync: { $($sync_def)* }
            }
        };

        (
            $test_mod_name:ident,
            trait $base_trait:path { $($base_impl:tt)* },
            type $base_type:ty,
            non_sync: { $($non_sync_def:tt)+ },
            sync: { $($sync_def:tt)+ }
        ) => {
            mod $test_mod_name {
                // Downcast
                test_mod!(
                    @test
                    $test_mod_name,
                    test_name: test_non_sync,
                    trait $base_trait { $($base_impl)* },
                    type $base_type,
                    { $($non_sync_def)+ },
                    []);

                // DowncastSend
                test_mod!(
                    @test
                    $test_mod_name,
                    test_name: test_send,
                    trait $base_trait { $($base_impl)* },
                    type $base_type,
                    { subst_downcast_send! { $($non_sync_def)+ } },
                    [{
                        // Downcast to include Send.
                        let base: $crate::__alloc::boxed::Box<$base_type> = $crate::__alloc::boxed::Box::new(Foo(42));
                        fn impls_send<T: ::std::marker::Send>(_a: T) {}
                        impls_send(base.into_any_send());
                    }]);

                // DowncastSync
                test_mod!(
                    @test
                    $test_mod_name,
                    test_name: test_sync,
                    trait $base_trait { $($base_impl)* },
                    type $base_type,
                    { $($sync_def)+ },
                    [{
                        // Downcast to include Send.
                        let base: $crate::__alloc::boxed::Box<$base_type> = $crate::__alloc::boxed::Box::new(Foo(42));
                        fn impls_send<T: ::std::marker::Send>(_a: T) {}
                        impls_send(base.into_any_send());
                        // Downcast to include Send + Sync.
                        let base: $crate::__alloc::boxed::Box<$base_type> = $crate::__alloc::boxed::Box::new(Foo(42));
                        fn impls_send_sync<T: ::std::marker::Send + ::std::marker::Sync>(_a: T) {}
                        impls_send_sync(base.into_any_sync());

                        // Fail to convert Arc<dyn Base> into Arc<Bar>.
                        let arc: $crate::__alloc::sync::Arc<$base_type> = $crate::__alloc::sync::Arc::new(Foo(42));
                        let res = arc.downcast_arc::<Bar>();
                        assert!(res.is_err());
                        let arc = res.unwrap_err();
                        // Convert Arc<dyn Base> into Arc<Foo>.
                        assert_eq!(
                            42, arc.downcast_arc::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);
                    }]);
            }
        };

        (
            @test
            $test_mod_name:ident,
            test_name: $test_name:ident,
            trait $base_trait:path { $($base_impl:tt)* },
            type $base_type:ty,
            { $($def:tt)+ },
            [ $($more_tests:block)* ]
        ) => {
            #[test]
            fn $test_name() {
                #[allow(unused_imports)]
                use super::super::{Downcast, DowncastSend, DowncastSync};

                // Should work even if standard objects (especially those in the prelude) are
                // aliased to something else.
                #[allow(dead_code)] struct Any;
                #[allow(dead_code)] struct Arc;
                #[allow(dead_code)] struct Box;
                #[allow(dead_code)] struct Option;
                #[allow(dead_code)] struct Result;
                #[allow(dead_code)] struct Rc;
                #[allow(dead_code)] struct Send;
                #[allow(dead_code)] struct Sync;

                // A trait that can be downcast.
                $($def)*

                // Concrete type implementing Base.
                #[derive(Debug)]
                struct Foo(u32);
                impl $base_trait for Foo { $($base_impl)* }
                #[derive(Debug)]
                struct Bar(f64);
                impl $base_trait for Bar { $($base_impl)* }

                // Functions that can work on references to Base trait objects.
                fn get_val(base: &$crate::__alloc::boxed::Box<$base_type>) -> u32 {
                    match base.downcast_ref::<Foo>() {
                        Some(val) => val.0,
                        None => 0
                    }
                }
                fn set_val(base: &mut $crate::__alloc::boxed::Box<$base_type>, val: u32) {
                    if let Some(foo) = base.downcast_mut::<Foo>() {
                        foo.0 = val;
                    }
                }

                let mut base: $crate::__alloc::boxed::Box<$base_type> = $crate::__alloc::boxed::Box::new(Foo(42));
                assert_eq!(get_val(&base), 42);

                // Try sequential downcasts.
                if let Some(foo) = base.downcast_ref::<Foo>() {
                    assert_eq!(foo.0, 42);
                } else if let Some(bar) = base.downcast_ref::<Bar>() {
                    assert_eq!(bar.0, 42.0);
                }

                set_val(&mut base, 6*9);
                assert_eq!(get_val(&base), 6*9);

                assert!(base.is::<Foo>());

                // Fail to convert Box<dyn Base> into Box<Bar>.
                let res = base.downcast::<Bar>();
                assert!(res.is_err());
                let base = res.unwrap_err();
                // Convert Box<dyn Base> into Box<Foo>.
                assert_eq!(
                    6*9, base.downcast::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);

                // Fail to convert Rc<dyn Base> into Rc<Bar>.
                let rc: $crate::__alloc::rc::Rc<$base_type> = $crate::__alloc::rc::Rc::new(Foo(42));
                let res = rc.downcast_rc::<Bar>();
                assert!(res.is_err());
                let rc = res.unwrap_err();
                // Convert Rc<dyn Base> into Rc<Foo>.
                assert_eq!(
                    42, rc.downcast_rc::<Foo>().map_err(|_| "Shouldn't happen.").unwrap().0);

                $($more_tests)*
            }
        };
        (
            $test_mod_name:ident,
            trait $base_trait:path { $($base_impl:tt)* },
            non_sync: { $($non_sync_def:tt)+ },
            sync: { $($sync_def:tt)+ }
        ) => {
            test_mod! {
                $test_mod_name,
                trait $base_trait { $($base_impl:tt)* },
                type $base_trait,
                non_sync: { $($non_sync_def)* },
                sync: { $($sync_def)* }
            }
        };

    }

    test_mod!(non_generic, trait Base {},
        non_sync: {
            trait Base: Downcast {}
            impl_downcast!(Base);
        },
        sync: {
            trait Base: DowncastSync {}
            impl_downcast!(sync Base);
        });

    test_mod!(generic, trait Base<u32> {},
        non_sync: {
            trait Base<T>: Downcast {}
            impl_downcast!(Base<T>);
        },
        sync: {
            trait Base<T>: DowncastSync {}
            impl_downcast!(sync Base<T>);
        });

    test_mod!(constrained_generic, trait Base<u32> {},
        non_sync: {
            trait Base<T: Copy>: Downcast {}
            impl_downcast!(Base<T> where T: Copy);
        },
        sync: {
            trait Base<T: Copy>: DowncastSync {}
            impl_downcast!(sync Base<T> where T: Copy);
        });

    test_mod!(associated,
        trait Base { type H = f32; },
        type dyn Base<H=f32>,
        non_sync: {
            trait Base: Downcast { type H; }
            impl_downcast!(Base assoc H);
        },
        sync: {
            trait Base: DowncastSync { type H; }
            impl_downcast!(sync Base assoc H);
        });

    test_mod!(constrained_associated,
        trait Base { type H = f32; },
        type dyn Base<H=f32>,
        non_sync: {
            trait Base: Downcast { type H: Copy; }
            impl_downcast!(Base assoc H where H: Copy);
        },
        sync: {
            trait Base: DowncastSync { type H: Copy; }
            impl_downcast!(sync Base assoc H where H: Copy);
        });

    test_mod!(param_and_associated,
        trait Base<u32> { type H = f32; },
        type dyn Base<u32, H=f32>,
        non_sync: {
            trait Base<T>: Downcast { type H; }
            impl_downcast!(Base<T> assoc H);
        },
        sync: {
            trait Base<T>: DowncastSync { type H; }
            impl_downcast!(sync Base<T> assoc H);
        });

    test_mod!(constrained_param_and_associated,
        trait Base<u32> { type H = f32; },
        type dyn Base<u32, H=f32>,
        non_sync: {
            trait Base<T: Clone>: Downcast { type H: Copy; }
            impl_downcast!(Base<T> assoc H where T: Clone, H: Copy);
        },
        sync: {
            trait Base<T: Clone>: DowncastSync { type H: Copy; }
            impl_downcast!(sync Base<T> assoc H where T: Clone, H: Copy);
        });

    test_mod!(concrete_parametrized, trait Base<u32> {},
        non_sync: {
            trait Base<T>: Downcast {}
            impl_downcast!(concrete Base<u32>);
        },
        sync: {
            trait Base<T>: DowncastSync {}
            impl_downcast!(sync concrete Base<u32>);
        });

    test_mod!(concrete_associated,
        trait Base { type H = u32; },
        type dyn Base<H=u32>,
        non_sync: {
            trait Base: Downcast { type H; }
            impl_downcast!(concrete Base assoc H=u32);
        },
        sync: {
            trait Base: DowncastSync { type H; }
            impl_downcast!(sync concrete Base assoc H=u32);
        });

    test_mod!(concrete_parametrized_associated,
        trait Base<u32> { type H = f32; },
        type dyn Base<u32, H=f32>,
        non_sync: {
            trait Base<T>: Downcast { type H; }
            impl_downcast!(concrete Base<u32> assoc H=f32);
        },
        sync: {
            trait Base<T>: DowncastSync { type H; }
            impl_downcast!(sync concrete Base<u32> assoc H=f32);
        });
}
