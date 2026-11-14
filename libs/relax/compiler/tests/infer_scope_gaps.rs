//! Inference and scope rules a large application's crates rely on, as rustc applies them (relax PR
//! 193: each case names the crate it came from).
use std::future::Future;
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};

struct Name(String);
impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
impl AsRef<[u8]> for Name {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// sqlez bindable.rs, json_schema_store: `x.as_ref() as &Path` with several `AsRef` impls; the
/// cast's coercion at the end of typeck (rustc check_casts) picks the impl.
#[test]
fn a_cast_target_decides_an_open_reference() {
    let p = PathBuf::from("abc");
    assert_eq!((p.as_ref() as &Path).as_os_str().len(), 3);
    let n = Name("ab".into());
    assert!(n.as_ref() as &str == "ab");
    assert_eq!((n.as_ref() as &[u8]).len(), 2);
}

/// agent_ui native_notification.rs: an async block's output is known only from the tail after an
/// endless `loop` (the tail is coerced although unreachable).
#[test]
fn an_unreachable_tail_types_an_async_block() {
    let serve = async {
        let _n: u32 = "5".parse()?;
        loop {
            let _m: u32 = "x".parse()?;
        }
        #[allow(unreachable_code)]
        Ok::<(), std::num::ParseIntError>(())
    };
    let mut cx = Context::from_waker(Waker::noop());
    let mut f = std::pin::pin!(serve);
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(r) => assert!(r.is_err()),
        Poll::Pending => panic!("pending"),
    }
}

mod ext {
    pub trait Context<T> {
        fn with_context(self, f: impl FnOnce() -> String) -> Result<T, String>;
    }
    impl<T> Context<T> for Option<T> {
        fn with_context(self, f: impl FnOnce() -> String) -> Result<T, String> {
            self.ok_or_else(f)
        }
    }
}
mod gui {
    pub struct Context;
}
mod parent {
    #[allow(unused_imports)]
    use crate::ext::Context as _;
    /// agent_ui session_fork.rs: the parent's `use anyhow::Context as _` reaches the child through
    /// `use super::*`; the child's explicit `Context` (gpui's) cannot shadow an unnamed import.
    pub mod shadowed {
        use super::*;
        use crate::gui::Context;
        pub fn f(o: Option<u8>) -> Result<u8, String> {
            let _c: Option<Context> = None;
            o.with_context(|| "none".into())
        }
    }
}

#[test]
fn a_glob_imported_underscore_trait_is_not_shadowed() {
    assert_eq!(parent::shadowed::f(Some(3)), Ok(3));
    assert_eq!(parent::shadowed::f(None), Err("none".to_string()));
}

struct Cursor<D>(D);
impl<D: Default> Cursor<D> {
    fn new() -> Self {
        Cursor(D::default())
    }
    fn start(&self) -> &D {
        &self.0
    }
}
struct Chunks {
    cursor: Cursor<usize>,
    offset: usize,
}

/// rope.rs Chunks::new: `offset - cursor.start()` with `start(): &D` is the operator trait
/// `usize: Sub<&D>` (a reference is no primitive operand); `D` comes from the struct later, or
/// from the one impl with a reference on the right.
#[test]
fn a_reference_right_operand_is_an_operator_trait_call() {
    let cursor = Cursor::new();
    let end: usize = 5;
    let offset = end - cursor.start();
    let chunks = Chunks { cursor, offset };
    assert_eq!((chunks.offset, *chunks.cursor.start()), (5, 0));
    let only = Cursor::new();
    assert_eq!(4usize - only.start(), 4);
}

trait Inst {
    fn rc_for_type(t: u8) -> u8;
}
trait Spec {
    type I: Inst;
}
struct A64;
impl Inst for A64 {
    fn rc_for_type(t: u8) -> u8 {
        t + 1
    }
}
struct Machine;
impl Spec for Machine {
    type I = A64;
}

fn spill<M: Spec>(t: u8) -> (u8, u8) {
    (<M>::I::rc_for_type(t), M::I::rc_for_type(t))
}

/// cranelift-codegen machinst/abi.rs Callee::gen_spill: `<M>::I::rc_for_type(ty)`, a qualified
/// self type without a trait, then its associated type, then that type's function.
#[test]
fn a_qualified_self_type_reaches_its_associated_type() {
    assert_eq!(spill::<Machine>(3), (4, 4));
}

struct Props<'a>(std::slice::Iter<'a, (String, u8)>);

impl<'a> Iterator for Props<'a> {
    type Item = (&'a str, &'a u8);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let pair = self.0.next()?;
            if pair.1 == 0 {
                continue;
            }
            let (ref key, val) = pair;
            break Some((key, val));
        }
    }
}

/// ec4rs 1.2 properties/iter.rs: a `loop`'s expected type is what its `break` values coerce to:
/// `key` (a `&String`) becomes `&str` inside `Some((key, val))`.
#[test]
fn a_loop_break_value_coerces_to_the_expected_type() {
    let v = vec![("a".to_string(), 0u8), ("b".to_string(), 2)];
    let got: Vec<(&str, &u8)> = Props(v.iter()).collect();
    assert_eq!(got, [("b", &2)]);
    let s = String::from("x");
    let twice: Option<&str> = loop {
        if s.is_empty() {
            break None;
        }
        break Some(&s);
    };
    assert_eq!(twice, Some("x"));
}
