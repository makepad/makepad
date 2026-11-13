// A derive whose `#[proc_macro_derive]` fn a dependency's macro declares, imported by path and
// through a re-export as stylo imports `MallocSizeOf` (`mod derives { pub(crate) use
// malloc_size_of_derive::MallocSizeOf; }`, then `use crate::derives::MallocSizeOf;`): both
// imports were unresolved, the derive not being among the macro crate's entries.
mod derives {
    pub(crate) use ddm_mac::Named;
}

mod inner {
    use crate::derives::Named;

    #[derive(Named)]
    pub struct Inner {
        #[named_skip]
        pub _seed: u8,
    }
}

use ddm_mac::Named;

pub trait NamedTrait {
    fn name(&self) -> &'static str;
}

#[derive(Named)]
struct Outer;

fn main() {
    println!("declared derive {} {}", Outer.name(), inner::Inner { _seed: 1 }.name());
}
