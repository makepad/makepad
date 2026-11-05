// The crate root re-exports a proc-macro crate by glob (the derive `Item`) and a trait `Item` by a
// single import. `use crate::Item` in another module takes both: rustc's import resolution
// determines the macro namespace through the glob, so nothing is left undetermined.
pub use ibdg_der::*;

pub mod item {
    pub trait Item {
        fn id(&self) -> u32;
    }
}

pub use crate::item::Item;

mod user {
    use crate::Item;

    pub fn id_of<T: Item>(t: &T) -> u32 {
        t.id()
    }
}

#[derive(Item)]
pub struct Seven;

impl Item for Seven {
    fn id(&self) -> u32 {
        7
    }
}

pub fn run() -> u32 {
    user::id_of(&Seven)
}
