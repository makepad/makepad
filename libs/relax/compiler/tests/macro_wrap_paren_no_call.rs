// rand 0.8's impl_fill_arrays!: `(!div $n:expr, ..)` arms after `($n:expr, ..)` arms; a recursive call
// passes `!div $n / 2` with $n an already-captured expression
trait Len { const L: usize; }
macro_rules! lens {
    ($n:expr,) => {};
    ($n:expr, $N:ident) => {
        impl Len for [u8; $n] { const L: usize = $n; }
    };
    ($n:expr, $N:ident, $($NN:ident,)*) => {
        lens!($n, $N);
        lens!($n - 1, $($NN,)*);
    };
    (!div $n:expr,) => {};
    (!div $n:expr, $N:ident, $($NN:ident,)*) => {
        lens!($n, $N);
        lens!(!div $n / 2, $($NN,)*);
    };
}
lens!(3, N, N, N,);
lens!(!div 64, N, N, N,);

#[test]
fn div_arms_pick_the_literal_arm() {
    assert_eq!(<[u8; 3] as Len>::L, 3);
    assert_eq!(<[u8; 1] as Len>::L, 1);
    assert_eq!(<[u8; 64] as Len>::L, 64);
    assert_eq!(<[u8; 32] as Len>::L, 32);
    assert_eq!(<[u8; 16] as Len>::L, 16);
}
