//~ none
// A const parameter is in the value namespace alone (rustc_resolve's generics: a type parameter
// in the type rib, a const parameter in the value rib): `N::of` in a fn with `const N` names the
// module `N`'s item, where a type parameter of a module's or trait's name would hide it (relax
// issue 32).
mod N {
    pub fn of(n: usize) -> usize {
        n * 10
    }
}

fn spans<const N: usize>() -> usize {
    N + N::of(N)
}

fn main() {
    assert_eq!(spans::<2>(), 22);
}
