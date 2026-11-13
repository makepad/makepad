// Pin::map_unchecked (stable since 1.33): a pinned shared reference projected to a part of its
// pointee. tracing 0.1's `InstrumentedProjRef::span_and_inner_pin_ref` (src/instrument.rs) calls
// `self.inner.map_unchecked(|v| &**v)` on a `Pin<&ManuallyDrop<T>>`; the std here had only the
// `_mut` form, so that failed with "no method named `map_unchecked` found". With it, the other
// reference constructors core has: into_ref, as_deref_mut, static_ref and static_mut.
use std::future::Future;
use std::marker::PhantomPinned;
use std::mem::ManuallyDrop;
use std::pin::{pin, Pin};
use std::task::{Context, Poll, Waker};

struct Span(&'static str);

// tracing's shape: a pinned projection holding the wrapped value in a ManuallyDrop
struct Instrumented<T> {
    inner: ManuallyDrop<T>,
    span: Span,
}
struct ProjRef<'a, T> {
    inner: Pin<&'a ManuallyDrop<T>>,
    span: &'a Span,
}
impl<T> Instrumented<T> {
    fn project_ref(self: Pin<&Self>) -> ProjRef<'_, T> {
        let this = self.get_ref();
        ProjRef { inner: unsafe { Pin::new_unchecked(&this.inner) }, span: &this.span }
    }
}
impl<'a, T> ProjRef<'a, T> {
    fn span_and_inner_pin_ref(self) -> (&'a Span, Pin<&'a T>) {
        let inner = unsafe { self.inner.map_unchecked(|v| &**v) };
        (self.span, inner)
    }
}

struct NotUnpin {
    value: u64,
    bytes: [u8; 4],
    _pin: PhantomPinned,
}

struct Ready(u32);
impl Future for Ready {
    type Output = u32;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
        Poll::Ready(self.0)
    }
}

#[test]
fn map_unchecked_projects_a_shared_pin() {
    let wrapped = Instrumented { inner: ManuallyDrop::new(NotUnpin { value: 9, bytes: [1, 2, 3, 4], _pin: PhantomPinned }), span: Span("request") };
    let pinned = unsafe { Pin::new_unchecked(&wrapped) };
    let (span, inner): (&Span, Pin<&NotUnpin>) = pinned.project_ref().span_and_inner_pin_ref();
    assert_eq!(span.0, "request");
    assert_eq!(inner.value, 9);
    // the projection points into the same value, with the outer lifetime
    assert!(std::ptr::eq(inner.get_ref(), &*wrapped.inner));

    // to a field, to an unsized part, and with a closure that captures
    let field: Pin<&u64> = unsafe { inner.map_unchecked(|n| &n.value) };
    assert_eq!(*field, 9);
    let slice: Pin<&[u8]> = unsafe { inner.map_unchecked(|n| &n.bytes[..]) };
    assert_eq!(slice.get_ref(), &[1, 2, 3, 4]);
    let skip = 2;
    let tail: Pin<&[u8]> = unsafe { slice.map_unchecked(move |b| &b[skip..]) };
    assert_eq!(&*tail, &[3, 4]);
    let owned = String::from("pinned");
    let text: Pin<&str> = unsafe { Pin::new(&owned).map_unchecked(|s| s.as_str()) };
    assert_eq!(text.get_ref(), "pinned");
}

#[test]
fn into_ref_and_as_deref_mut() {
    let mut value = NotUnpin { value: 5, bytes: [0; 4], _pin: PhantomPinned };
    let mut pinned: Pin<&mut NotUnpin> = unsafe { Pin::new_unchecked(&mut value) };
    unsafe { pinned.as_mut().get_unchecked_mut().value += 1 };
    let shared: Pin<&NotUnpin> = pinned.into_ref();
    assert_eq!(shared.value, 6);
    assert_eq!(unsafe { shared.map_unchecked(|n| &n.bytes) }.len(), 4);

    // a pinned pointer that is itself pinned: through both to the future
    let mut boxed: Pin<Box<Ready>> = Box::pin(Ready(41));
    let outer: Pin<&mut Pin<Box<Ready>>> = Pin::new(&mut boxed);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(outer.as_deref_mut().poll(&mut cx), Poll::Ready(41));
    let mut local = pin!(Ready(7));
    let through: Pin<&mut Ready> = Pin::new(&mut local).as_deref_mut();
    assert_eq!(through.poll(&mut cx), Poll::Ready(7));
}

#[test]
fn static_references_are_pinned_for_good() {
    static FIXED: NotUnpin = NotUnpin { value: 77, bytes: [9; 4], _pin: PhantomPinned };
    let p: Pin<&'static NotUnpin> = Pin::static_ref(&FIXED);
    assert_eq!(p.value, 77);
    let bytes: Pin<&'static [u8; 4]> = unsafe { p.map_unchecked(|n| &n.bytes) };
    assert_eq!(bytes.get_ref(), &[9; 4]);
    const PINNED: Pin<&'static str> = Pin::static_ref("const");
    assert_eq!(PINNED.len(), 5);

    let leaked: &'static mut NotUnpin = Box::leak(Box::new(NotUnpin { value: 1, bytes: [0; 4], _pin: PhantomPinned }));
    let mut m: Pin<&'static mut NotUnpin> = Pin::static_mut(leaked);
    unsafe { m.as_mut().get_unchecked_mut().value = 2 };
    assert_eq!(m.into_ref().value, 2);
}
