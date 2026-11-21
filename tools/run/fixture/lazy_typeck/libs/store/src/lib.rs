// lazy_typeck.sh: the impl of lazy_typeck_dep::Engine without the `at` the trait gained (E0046). A
// call through it reports the E0046 at the call, not an internal error, and parks until a live edit
// adds `at`; `makepad check` reports it.
pub struct Store(pub Vec<usize>);

impl lazy_typeck_dep::Engine for Store {
    fn len(&self) -> usize {
        self.0.len()
    }
    // trait-park replaces this line with Store::at.
}

pub fn first_of(s: &Store) -> usize {
    lazy_typeck_dep::first(s)
}

// E0050 (a parameter fewer than the trait's) and E0407 (a method the trait does not have)
pub struct Short;

impl lazy_typeck_dep::Engine for Short {
    fn len(&self) -> usize {
        0
    }
    fn at(&self) -> usize {
        0
    }
    fn extra(&self) -> usize {
        7
    }
}
