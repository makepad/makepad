// Macros only an expansion of this crate defines: `#[macro_export]` puts them at the crate root,
// where a dependent names them by path, imports them (by name or glob) or takes them with
// `#[macro_use]`; a local one is in textual scope in the modules declared after it, and a
// `#[macro_use]` module's in the modules after that one.
macro_rules! make {
    ($name:ident, $v:expr) => {
        #[macro_export]
        macro_rules! $name {
            () => {
                $v
            };
        }
    };
}

macro_rules! local {
    ($name:ident, $v:expr) => {
        macro_rules! $name {
            () => {
                $v
            };
        }
    };
}

make!(answer, 42);
make!(other, 7);
make!(third, 3);
local!(seen_later, 11);

#[macro_use]
mod defs;
mod exports;
mod uses;

pub fn inner() -> u32 {
    uses::get()
}
