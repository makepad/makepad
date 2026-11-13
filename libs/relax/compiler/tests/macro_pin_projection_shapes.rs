// relax issue 79: the calls of a pin-projection macro in a workspace's async dependencies (an HTTP
// body wrapper, a runtime's local task set, a client's future enum, a retry future, an event
// listener) matched no rule of its generics parser, and every crate holding one failed to load.
// The reporter's revision sat between the matcher port (relax issue 21) and relax issue 26's fix.
// The macro below has that pipeline's matcher shapes: projection attributes taken one at a time
// into bracket groups, the visibility forwarded twice (the item's, and the projection's with `pub`
// lowered to `pub(crate)`), generics with lifetime bounds, a trait bound, `?Sized` and defaults, a
// where clause, struct and enum bodies with `#[pin]` fields, and an impl after the body.
use std::marker::PhantomData;

macro_rules! projected {
    ($($tt:tt)*) => {
        projected_attrs! { [] [] [] [] $($tt)* }
    };
}

macro_rules! projected_attrs {
    ([] [$($pr:ident)?] [$($un:ident)?] [$($attrs:tt)*] #[project = $pm:ident] $($tt:tt)*) => {
        projected_attrs! { [$pm] [$($pr)?] [$($un)?] [$($attrs)*] $($tt)* }
    };
    ([$($pm:ident)?] [] [$($un:ident)?] [$($attrs:tt)*] #[project_ref = $pr:ident] $($tt:tt)*) => {
        projected_attrs! { [$($pm)?] [$pr] [$($un)?] [$($attrs)*] $($tt)* }
    };
    ([$($pm:ident)?] [$($pr:ident)?] [] [$($attrs:tt)*] #[project(!$un:ident)] $($tt:tt)*) => {
        projected_attrs! { [$($pm)?] [$($pr)?] [$un] [$($attrs)*] $($tt)* }
    };
    ([$($pm:ident)?] [$($pr:ident)?] [$($un:ident)?] [$($attrs:tt)*] #[$($attr:tt)*] $($tt:tt)*) => {
        projected_attrs! { [$($pm)?] [$($pr)?] [$($un)?] [$($attrs)* #[$($attr)*]] $($tt)* }
    };
    ([$($pm:ident)?] [$($pr:ident)?] [$($un:ident)?] [$($attrs:tt)*] pub $($tt:tt)*) => {
        projected_generics! { [$($pm)?] [$($pr)?] [$($un)?] [$($attrs)*] [pub $($tt)*] [pub(crate)] }
    };
    ([$($pm:ident)?] [$($pr:ident)?] [$($un:ident)?] [$($attrs:tt)*] $vis:vis $kind:ident $name:ident $($tt:tt)*) => {
        projected_generics! { [$($pm)?] [$($pr)?] [$($un)?] [$($attrs)*] [$vis $kind $name $($tt)*] [$vis] }
    };
}

macro_rules! projected_generics {
    (
        [$($pm:ident)?] [$($pr:ident)?] [$($un:ident)?] [$($attrs:tt)*]
        [$vis:vis $kind:ident $name:ident
            $(<
                $($lt:lifetime $(: $lt_bound:lifetime)?),* $(,)?
                $($g:ident
                    $(: $g_bound:path)?
                    $(: ?$g_unsized:path)?
                    $(: $g_lt_bound:lifetime)?
                    $(= $g_default:ty)?
                ),* $(,)?
            >)?
            $(where
                $($w_ty:ty
                    $(: $w_bound:path)?
                    $(: ?$w_unsized:path)?
                    $(: $w_lt_bound:lifetime)?
                ),* $(,)?
            )?
            { $($body:tt)* }
            $($(#[$tail_attr:meta])* impl $($tail:tt)*)?
        ]
        [$proj_vis:vis]
    ) => {
        projected_item! {
            [$($attrs)*] [$vis] $kind $name
            [$(<$($lt $(: $lt_bound)?,)* $($g $(: $g_bound)? $(: ?$g_unsized)? $(: $g_lt_bound)? $(= $g_default)?),*>)?]
            [$(where $($w_ty $(: $w_bound)? $(: ?$w_unsized)? $(: $w_lt_bound)?),*)?]
            { $($body)* }
        }
        impl $(<$($lt $(: $lt_bound)?,)* $($g $(: $g_bound)? $(: ?$g_unsized)? $(: $g_lt_bound)?),*>)? $name $(<$($lt,)* $($g),*>)?
            $(where $($w_ty $(: $w_bound)? $(: ?$w_unsized)? $(: $w_lt_bound)?),*)?
        {
            $proj_vis fn shape() -> String {
                format!(
                    "{} {}: lifetimes {}, generics {}, where {}, project [{}] [{}] [{}], tail {}, projection vis [{}]",
                    stringify!($kind),
                    stringify!($name),
                    0 $($(+ { let _ = stringify!($lt); 1 })*)?,
                    0 $($(+ { let _ = stringify!($g); 1 })*)?,
                    0 $($(+ { let _ = stringify!($w_ty); 1 })*)?,
                    concat!("" $(, stringify!($pm))?),
                    concat!("" $(, stringify!($pr))?),
                    concat!("" $(, stringify!($un))?),
                    0 $(+ { let _ = stringify!($($tail)*); 1 })?,
                    stringify!($proj_vis),
                )
            }
        }
        $($(#[$tail_attr])* impl $($tail)*)?
    };
}

macro_rules! projected_item {
    ([$($attrs:tt)*] [$vis:vis] struct $name:ident [$($decl:tt)*] [$($where_:tt)*]
        { $($(#[$pin:ident])? $field_vis:vis $field:ident : $field_ty:ty),* $(,)? }) => {
        $($attrs)* $vis struct $name $($decl)* $($where_)* { $($field_vis $field: $field_ty),* }
    };
    ([$($attrs:tt)*] [$vis:vis] enum $name:ident [$($decl:tt)*] [$($where_:tt)*]
        { $($variant:ident { $($(#[$pin:ident])? $field:ident : $field_ty:ty),* $(,)? }),* $(,)? }) => {
        $($attrs)* $vis enum $name $($decl)* $($where_)* { $($variant { $($field: $field_ty),* }),* }
    };
}

trait Tail {
    fn tail(&self) -> u8;
}

projected! {
    #[project = BodyProj]
    #[derive(Debug)]
    pub enum Body<T, F> {
        Streaming { #[pin] inner: T, trailers: Option<F> },
        Done { trailers: Option<F> },
    }
}

projected! {
    struct RunUntil<'a, T>
    where
        T: Clone,
    {
        set: &'a u8,
        #[pin]
        future: T,
    }
}

projected! {
    #[project(!Unpin)]
    #[derive(Debug)]
    pub struct Listener<T: ?Sized = u8, B: Clone = ()> {
        #[pin]
        listener: PhantomData<T>,
        pub b: B,
    }
}

projected! {
    #[project = LazyProj]
    #[project_ref = LazyRef]
    enum Lazy<F, R> {
        Init { func: F },
        Fut { #[pin] fut: R },
    }
}

projected! {
    pub(crate) struct Retry<'a: 'b, 'b, P>
    where
        P: 'a,
    {
        policy: &'b P,
        life: &'a u8,
    }
    #[allow(clippy::needless_lifetimes)]
    impl<'a: 'b, 'b, P> Tail for Retry<'a, 'b, P>
    where
        P: 'a,
    {
        fn tail(&self) -> u8 {
            *self.life + 9
        }
    }
}

#[test]
fn pin_projection_shapes_match_their_generics_parser() {
    assert_eq!(Body::<u8, u16>::shape(), "enum Body: lifetimes 0, generics 2, where 0, project [BodyProj] [] [], tail 0, projection vis [pub(crate)]", "a pub enum with a projection name");
    let body = Body::<u8, u16>::Streaming { inner: 1, trailers: Some(2) };
    assert!(matches!(body, Body::Streaming { inner: 1, trailers: Some(2) }) && !matches!(Body::<u8, u16>::Done { trailers: None }, Body::Streaming { .. }), "{body:?}");
    assert_eq!(RunUntil::<u8>::shape(), "struct RunUntil: lifetimes 1, generics 1, where 1, project [] [] [], tail 0, projection vis []", "a lifetime and a where clause, no visibility");
    let set = 3u8;
    let run = RunUntil { set: &set, future: 4u8 };
    assert_eq!(*run.set + run.future, 7, "the fields lose their #[pin]");
    assert_eq!(<Listener>::shape(), "struct Listener: lifetimes 0, generics 2, where 0, project [] [] [Unpin], tail 0, projection vis [pub(crate)]", "?Sized and defaults, not Unpin");
    let listener: Listener = Listener { listener: PhantomData, b: () };
    assert_eq!(format!("{listener:?}"), "Listener { listener: PhantomData<u8>, b: () }", "the derive passed on");
    assert_eq!(Lazy::<u8, u16>::shape(), "enum Lazy: lifetimes 0, generics 2, where 0, project [LazyProj] [LazyRef] [], tail 0, projection vis []", "two projection names");
    let lazy = [Lazy::<u8, u16>::Init { func: 5 }, Lazy::Fut { fut: 6 }];
    assert_eq!(lazy.iter().map(|l| match l { Lazy::Init { func } => *func as u16, Lazy::Fut { fut } => *fut }).sum::<u16>(), 11, "both variants");
    assert_eq!(Retry::<u8>::shape(), "struct Retry: lifetimes 2, generics 1, where 1, project [] [] [], tail 1, projection vis [pub(crate)]", "lifetime bounds, a where clause and an impl after the body");
    let (policy, life) = (0u8, 1u8);
    let retry = Retry { policy: &policy, life: &life };
    assert_eq!(retry.tail() + *retry.policy, 10, "the impl after the body");
}
