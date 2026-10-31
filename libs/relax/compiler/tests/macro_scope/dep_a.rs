// a crate exporting `trace!` that the user crate does not import
#[macro_export]
macro_rules! trace { ($($t:tt)*) => { "dep_a::trace" } }
#[macro_export]
macro_rules! log { () => { "dep_a::log" } }
#[macro_export]
macro_rules! local_use { () => { "dep_a::local_use" } }
pub fn touch() {}
