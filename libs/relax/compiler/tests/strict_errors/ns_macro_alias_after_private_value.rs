//~ none
mod values { pub fn label() {} }
mod bridge {
    use crate::values::label;
    pub use std::stringify as label;

    pub fn call() { label(); }
}

use bridge::*;

fn main() {
    call();
    let _ = label!(ok);
}
