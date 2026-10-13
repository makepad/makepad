// exports its macro as `__trace` and re-exports it under `trace` from a module
#[macro_export]
macro_rules! __trace { ($($t:tt)*) => { "dep_b::trace" } }
pub mod log {
    pub use crate::__trace as trace;
}
