// Boot batch 2 (objc-sys paths): macro names vs local bindings in macro hygiene (B14), a foreign fn
// as a fn pointer (B16), `use m::{self}` (B17), impls for fn pointer types + a projection-rooted
// type-relative path (B18), `<T>::item` reaching trait items (B19). rustc: all pass.

// B14: msg_send!'s `let sel = sel!(..)`
macro_rules! sel {
    ($x:expr) => {
        $x + 1
    };
}
macro_rules! send {
    ($x:expr) => {{
        let sel = sel!($x);
        sel
    }};
}

#[test]
fn macro_name_beside_local() {
    assert!(send!(2) == 3);
}

// B16: `let imp: Imp = objc_msgSend;` then transmute and call
type Imp = unsafe extern "C" fn();
extern "C" {
    fn labs();
}

#[test]
fn foreign_fn_pointer() {
    let imp: Imp = labs;
    let g: unsafe extern "C" fn(i64) -> i64 = unsafe { core::mem::transmute(imp) };
    assert!(unsafe { g(-3) } == 3);
    let p = labs as *const ();
    assert!(p == imp as *const ());
}

// B17 (2018 form): `use m::{self}` binds the module
mod outer {
    pub mod inner {
        pub fn k() -> u32 {
            4
        }
    }
}
mod user {
    use crate::outer::inner::{self};
    pub fn f() -> u32 {
        inner::k()
    }
}

#[test]
fn use_self_in_group() {
    assert!(user::f() == 4);
}

// B18: objc-sys MethodImplementation for extern "C" fn types; `F::Args::count()`
trait Count {
    fn count() -> usize;
}
impl<A, B> Count for (A, B) {
    fn count() -> usize {
        2
    }
}
trait M {
    type Args: Count;
}
impl<T, R, A, B> M for extern "C" fn(&T, u32, A, B) -> R {
    type Args = (A, B);
}
fn nargs<F: M>(_f: F) -> usize {
    F::Args::count()
}
struct O;
extern "C" fn method(_o: &O, _s: u32, _a: *mut u8, _b: *mut u8) {}

#[test]
fn fn_pointer_impl_projection() {
    assert!(nargs(method as extern "C" fn(&O, u32, *mut u8, *mut u8)) == 2);
}

// B19: `<*mut Object>::encode()`
trait E {
    fn enc() -> u32;
}
impl E for *mut u8 {
    fn enc() -> u32 {
        5
    }
}
struct S;
impl S {
    fn three() -> u32 {
        3
    }
}

#[test]
fn qself_type_relative() {
    assert!(<*mut u8>::enc() + <S>::three() == 8);
}
