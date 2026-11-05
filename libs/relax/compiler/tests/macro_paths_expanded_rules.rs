// Macro paths and macro-made macros as rustc resolves them: path-qualified built-in macros
// (`core::assert!`, `std::debug_assert_eq!`, `::std::dbg!`, through an `extern crate` alias), built-in derives
// written as paths, `macro_rules!` definitions that an expansion makes (in textual scope from the
// invocation on, and shadowed by a later definition of the same name),
// `.await` inside a macro's format arguments, and a negative const argument in derive input.
// Run with `makepad test` and rustc --test; lines "| " are compared.
extern crate core as kore;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

mod made {
    // `make` is defined at the end of the file and reached by path: the `pick` it makes is in
    // textual scope from this invocation on, not from where `make`'s body was written
    crate::make!(pick, 1);
    pub fn first() -> i32 {
        pick!()
    }
    macro_rules! pick {
        () => {
            2
        };
    }
    pub fn second() -> i32 {
        pick!()
    }
}

#[test]
fn expanded_definitions_follow_their_invocation() {
    println!("| {} {}", made::first(), made::second());
}

#[test]
fn path_qualified_builtin_macros() {
    core::assert!(1 + 1 == 2);
    ::core::assert_eq!(2, 1 + 1, "sum");
    std::debug_assert_eq!(3, 1 + 2);
    std::debug_assert_ne!(3, 4);
    core::debug_assert!(true, "on {}", 1);
    kore::assert!(kore::cfg!(any()) == false);
    let s = std::format!("{}-{}", 1, 2);
    let d = ::std::dbg!(std::dbg!(2) + 1);
    println!("| {} {} {}", s, std::stringify!(a + b), d);
}

#[test]
fn debug_assert_checks_its_operands_through_partial_eq() {
    // borrowed operands of different types, as `assert_eq!` takes them
    let v = vec![1u8, 2];
    let a: &[u8] = &[1, 2];
    debug_assert_eq!(v, a);
    debug_assert_ne!(&v[..1], a);
    println!("| {}", cfg!(debug_assertions) as u8 + 7);
}

struct Neg<const N: i32>;
impl<const N: i32> std::fmt::Debug for Neg<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Neg({})", N)
    }
}
impl<const N: i32> Clone for Neg<N> {
    fn clone(&self) -> Self {
        Neg
    }
}

#[derive(core::fmt::Debug, ::core::clone::Clone, std::cmp::PartialEq)]
struct Pathed {
    a: u8,
}

#[derive(Debug, Clone)]
struct Holds(Neg<-1>, Neg<{ -2 }>);

#[test]
fn derives_written_as_paths_and_negative_const_args() {
    let p = Pathed { a: 3 };
    let q = p.clone();
    println!("| {:?} {} {:?}", q, p == q, Holds(Neg, Neg).clone());
}

fn noop_raw() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw()
    }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    RawWaker::new(std::ptr::null(), &VT)
}

fn block_on<F: Future>(mut f: F) -> F::Output {
    let waker = unsafe { Waker::from_raw(noop_raw()) };
    let mut cx = Context::from_waker(&waker);
    let mut f = unsafe { Pin::new_unchecked(&mut f) };
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
    }
}

struct Yield(bool);
impl Future for Yield {
    type Output = u32;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<u32> {
        if self.0 {
            Poll::Ready(5)
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

macro_rules! show {
    ($e:expr) => {
        format!("<{}>", $e)
    };
}

async fn shown() -> String {
    let a = show!(Yield(false).await);
    let b = format!("{} {}", a, show!(Yield(false).await + 1));
    b
}

#[test]
fn await_inside_expanded_format_arguments() {
    println!("| {}", block_on(shown()));
}

macro_rules! make_rules {
    ($name:ident, $v:expr) => {
        macro_rules! $name {
            () => {
                $v
            };
        }
    };
}
pub(crate) use make_rules as make;
