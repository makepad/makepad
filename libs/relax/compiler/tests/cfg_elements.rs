// `#[cfg(..)]` inside expressions and on parameters: array / tuple / call elements, match arms,
// closure and fn params, generic params (Reference expressions.md [expr.attr.restriction],
// conditional-compilation.md [cfg.attr]). rustc: all pass.
fn sum(v: &[u32]) -> u32 {
    let mut s = 0;
    for x in v {
        s += *x;
    }
    s
}

fn three(a: u32, #[cfg(windows)] w: u32, b: u32) -> u32 {
    a + b
}

struct G<#[cfg(windows)] W, T> {
    t: T,
    #[cfg(windows)]
    w: W,
}

#[test]
fn cfg_array_and_tuple_elements() {
    let a = [1u32, #[cfg(windows)] 100, 2, #[cfg(not(windows))] 4];
    assert!(a.len() == 3 && sum(&a) == 7);
    let t = (#[cfg(windows)] 9u8, 5u32, 6u32);
    assert!(t.0 + t.1 == 11);
}

#[test]
fn cfg_call_args_and_params() {
    assert!(three(1, #[cfg(windows)] 50, 2) == 3);
    let f = |x: u32, #[cfg(windows)] _w: u32| x * 2;
    assert!(f(4, #[cfg(windows)] 0) == 8);
}

#[test]
fn cfg_match_arms() {
    let v = 3u32;
    let r = match v {
        #[cfg(windows)]
        3 => 100,
        3 => 1,
        _ => 0,
    };
    assert!(r == 1);
}

#[test]
fn cfg_generic_params() {
    let g: G<u32> = G { t: 7 };
    assert!(g.t == 7);
}
