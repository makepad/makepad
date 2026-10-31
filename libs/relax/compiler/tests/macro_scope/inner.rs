// exports `log!` at its root
#[macro_export]
macro_rules! log { () => { "inner::log" } }
#[macro_export]
macro_rules! local_use { () => { "inner::local_use" } }
// `$crate::log!` inside an exported macro: inner's own log, from any calling crate
#[macro_export]
macro_rules! wrapped { () => { $crate::log!() } }
