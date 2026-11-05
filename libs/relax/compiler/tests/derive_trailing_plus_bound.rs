// A derive on a struct whose bound list ends in `+` (tls_codec 0.4.2 `impl_tls_vec_generic!`:
// `pub struct $name<T: $($bounds + )*>`): the trailing `+` is legal and the derive's impl header
// must not repeat it (`T: Zeroize + + Debug`) or close on it (`T: Zeroize + >`).
macro_rules! mk {
    ($name:ident $(, $bounds:ident)*) => {
        #[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name<T: $($bounds + )*> { pub v: T }
    };
}
mk!(S0);
mk!(S1, Copy);
mk!(S2, Copy, Send);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lt<'a, T: Clone + >(pub &'a [T]);

#[test]
fn derive_trailing_plus_bound() {
    assert_eq!(S0 { v: 1u8 }.clone(), S0 { v: 1u8 });
    assert_eq!(S1 { v: 3u8 }, S1 { v: 3u8 });
    assert!(S2 { v: 1u16 } < S2 { v: 2u16 });
    assert_eq!(S1::<u8>::default().v, 0);
    let a = [1u8, 2];
    assert_eq!(Lt(&a).clone(), Lt(&a));
    assert_eq!(format!("{:?}", S1 { v: 5u8 }), "S1 { v: 5 }");
}
