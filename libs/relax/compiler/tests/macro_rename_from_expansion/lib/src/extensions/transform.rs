// a second generated macro, one more re-export away (`pub use script::{script, Script};`),
// and a module of the same name beside it
use crate::subtags::script;
use crate::subtags::{subtag, Script, Subtag};

pub const TRUE_TVALUE: Subtag = subtag!("true");
pub const LATIN: Script = script!("Latn");

pub fn latin() -> Script {
    script::COMMON_OR_LATIN[1]
}
