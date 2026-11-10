// The same layout in the crate being run, and the dependency's macros under their public names.
macro_rules! named {
    ($name:ident, $internal:ident, $v:expr) => {
        #[macro_export]
        #[doc(hidden)]
        macro_rules! $internal {
            ($e:expr) => {
                $e * $v
            };
        }
        pub use $internal as $name;
    };
}

mod early;
mod names;

use mrx_lib::extensions::unicode;
use mrx_lib::subtags::{script, subtag};
use names::triple;

fn main() {
    let yes = unicode::is_true(subtag!("yes")) && unicode::is_true(unicode::TRUE_VALUE) && !unicode::is_true(mrx_lib::subtags::UNKNOWN);
    let latin = script!("Latn") == mrx_lib::extensions::transform::LATIN;
    println!("renamed macros {} {} {} | {} {}", mrx_lib::sum(), yes, latin, early::SIX, early::twice(triple!(5)));
}
