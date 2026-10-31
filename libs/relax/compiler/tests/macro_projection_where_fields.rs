use std::{future::Future, pin::Pin, task::{Context, Poll}};

trait Body { type Data; }
impl Body for () { type Data = u8; }

// The pin-project shape: parse the where clause, forward it as tt, then
// capture fields (including a qualified type starting with a glued <<).
macro_rules! project {
    (struct $name:ident<$($generic:ident),+> where $ty:ty: $bound:path, $body:tt) => {
        project_fields!([$name] [$($generic),+] [where $ty: $bound] $body);
    };
}

macro_rules! project_fields {
    ([$name:ident] [$($generic:ident),+] [where $($where_clause:tt)*] {
        $($(#[$pin:ident])? $vis:vis $field:ident: $ty:ty),+ $(,)?
    }) => {
        struct $name<$($generic),+> where $($where_clause)* {
            $($vis $field: $ty),+
        }
        const _: () = {
            projection_body! {
                [$name] [$($generic),+] [where $($where_clause)*]
                [$($vis $field: project_type!($(#[$pin])? $ty)),+]
            }
            impl<$($generic),+> $name<$($generic),+> where $($where_clause)* {
                fn project<'a>(self: Pin<&'a mut Self>) -> Projection<'a, $($generic),+> {
                    unsafe {
                        let Self { $($field),+ } = self.get_unchecked_mut();
                        Projection { $($field: project_value!($(#[$pin])? $field)),+ }
                    }
                }
            }
        };
    };
}

macro_rules! projection_body {
    ([$name:ident] [$($generic:ident),+] [where $($where_clause:tt)*] [$($body:tt)+]) => {
        // rustc_resolve build_reduced_graph.rs: try_resolve_visibility uses
        // nearest_parent_mod for private fields, including inside const blocks.
        struct Projection<'a, $($generic),+>
        where $name<$($generic),+>: 'a, $($where_clause)* {
            $($body)+
        }
    };
}

macro_rules! project_type {
    (#[pin] $ty:ty) => { Pin<&'a mut ($ty)> };
    ($ty:ty) => { &'a mut ($ty) };
}

macro_rules! project_value {
    (#[pin] $field:ident) => { Pin::new_unchecked($field) };
    ($field:ident) => { $field };
}

project! {
    struct Conn<T, B> where B: Body, {
        #[pin] fut: T,
        data: Option<<B as Body>::Data>,
    }
}

impl<T: Future<Output = u32>, B: Body> Future for Conn<T, B> {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
        let this = self.project();
        let _ = this.data;
        this.fut.poll(cx)
    }
}

#[test]
fn projection_fields_are_visible_outside_the_const_block() {
    let conn: Conn<_, ()> = Conn { fut: async { 42 }, data: Some(7) };
    let mut conn = std::pin::pin!(conn);
    let waker = std::task::Waker::noop();
    assert_eq!(conn.as_mut().poll(&mut Context::from_waker(waker)), Poll::Ready(42));
    assert_eq!(*conn.as_mut().project().data, Some(7));
}

#[test]
fn local_struct_fields_use_the_enclosing_module() {
    let value = { struct Local { field: u32 } Local { field: 9 } };
    assert_eq!(value.field, 9);
}
