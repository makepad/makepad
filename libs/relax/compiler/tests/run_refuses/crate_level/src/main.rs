// What only a crate-level check finds (check.rs crate_level_errors): rustc refuses each item below
// for a reason of its own, so `makepad run` must too, before main.
extern crate proc_macro;
use alloc::vec::Vec;
use std::alloc::System;

fn twice() {}
fn twice() {}

// one body's two statics of one name: E0428 of its block (a run took one for both)
fn counter() -> u32 {
    static COUNT: u32 = 0;
    static COUNT: u32 = 1;
    COUNT
}

mod a {
    #[macro_export]
    macro_rules! exported {
        () => {};
    }
}
mod b {
    #[macro_export]
    macro_rules! exported {
        () => {};
    }
}

mod m {
    mod hidden {
        pub trait Hidden {}
    }
}
struct S;
impl m::hidden::Hidden for S {}

struct Private;
pub trait Assoc {
    type A;
}
pub struct Public;
impl Assoc for Public {
    type A = Private;
}

enum E {
    #[inline]
    V,
}

fn param(#[inline] x: u32) -> u32 {
    x
}

extern "C" {
    safe fn abs(x: i32) -> i32;
}

#[no_mangle]
pub extern "C" fn dup_sym() {}
mod n {
    #[no_mangle]
    pub extern "C" fn dup_sym() {}
}

#[global_allocator]
static FIRST: System = System;
#[global_allocator]
static SECOND: System = System;

#[export_name = "main"]
pub fn other_main() {}

#[not_an_attribute]
fn unknown_attribute() {}

trait Plain {
    fn f();
}
impl Plain for S {
    pub fn f() {}
}

#[proc_macro]
pub fn pm(t: proc_macro::TokenStream) -> proc_macro::TokenStream {
    t
}

fn main() {
    println!("started");
    let x = #[allow(unused)] 1;
    let _ = (x, E::V, param(1), Vec::<u8>::new());
}
