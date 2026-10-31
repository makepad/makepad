//~ E0210
use std::fmt;
impl<T> fmt::Display for T { fn fmt(&self, _f: &mut fmt::Formatter) -> fmt::Result { Ok(()) } }
fn main() {}
