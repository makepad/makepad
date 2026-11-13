//! Literal suffixes in cfg-stripped code (relax issue 125): rustc reports a literal's suffix and an
//! integer's size where it lowers the literal, which a cfg-stripped item, module, statement, arm,
//! field or element never is, nor an arm `cfg_select!` did not choose. `"a "b" c"` lexes as the
//! string `"a "` with the suffix `b`, then `" c"`. Each of these stopped the crate's load
//! ("suffixes on string literals are invalid"). The predicates hold on the hosts agree runs on (unix).

#[cfg(any())]
compile_error!("either the "std" or "libm" feature must be enabled");

#[cfg(any())]
fn stripped() {
    let s = "a"x;
    let n = 1foo;
    let big = 340282366920938463463374607431768211456;
    match s {
        "b"y => {}
        _ => {}
    }
}

#[cfg(any())]
#[doc = "a"x]
fn stripped_with_its_attribute() {}

mod inner {
    #![cfg(any())]
    const A: &str = "a"x;
}

macro_rules! items {
    ($($t:tt)*) => { $($t)* };
}

items! {
    #[cfg(any())]
    fn stripped_in_an_expansion() { let s = "a"x; }
}

struct S {
    a: u8,
}

fn live() -> usize {
    #[cfg(any())]
    let s = "a"x;
    #[cfg(any())]
    compile_error!("a "b" c");
    let s = S { a: 1, #[cfg(any())] b: "x"y };
    let v = [1, #[cfg(any())] 2x];
    let n = match 1 {
        #[cfg(any())]
        1 => "a"x,
        _ => "bc",
    };
    cfg_select! {
        windows => { let _s = "a"x; }
        _ => {}
    }
    s.a as usize + v.len() + n.len()
}

#[test]
fn cfg_stripped_literals_keep_their_suffixes() {
    assert_eq!(live(), 4, "the live code around the stripped literals");
}
