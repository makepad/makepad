// A stack slot keeps its type's alignment whatever the function saves: an odd number of
// callee-saved registers below the frame's base left an align(16) local 8 bytes off on x64
// (GetThreadContext refused such a CONTEXT on Windows, movaps would fault).
use std::hint::black_box;

#[repr(align(16))]
struct Aligned([u64; 4]);

#[inline(never)]
fn off(a: &mut Aligned) -> usize {
    a.0[0] += 1;
    a as *mut Aligned as usize % 16
}

// `live` values stay in callee-saved registers across the calls
macro_rules! holding {
    ($name:ident, $($v:ident),*) => {
        #[inline(never)]
        fn $name(x: u64) -> (usize, u64) {
            $(let $v = black_box(x + 1);)*
            let mut a = Aligned([0; 4]);
            let o = off(&mut a);
            let o2 = off(&mut a);
            (o + o2, $($v +)* a.0[0])
        }
    };
}
holding!(one, a);
holding!(two, a, b);
holding!(three, a, b, c);
holding!(four, a, b, c, d);
holding!(five, a, b, c, d, e);

#[test]
fn an_aligned_local_stays_aligned() {
    for f in [one, two, three, four, five] {
        assert_eq!(f(1).0, 0);
    }
}
